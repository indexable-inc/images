//! Offline Setup Assistant bypass for a freshly installed macOS guest.
//!
//! A fresh install boots into Setup Assistant. On a host whose content filter
//! breaks the guest's TLS to Apple, the network-backed screens (Apple ID, Screen
//! Time) hang forever with greyed buttons, so the guest cannot be clicked past
//! them. This module performs, with the guest STOPPED, the proven host-side disk
//! edit that marks setup complete, so the next boot lands on a logged-in desktop.
//!
//! There are two layers of Setup Assistant:
//!
//! - **System** SA (language, country, account creation) is gated by
//!   `/var/db/.AppleSetupDone` on the guest's **Data** volume.
//! - **Per-user** SA ("MiniBuddy": Apple ID, Screen Time, Siri, appearance, …)
//!   is gated per user by `DidSee*` keys in
//!   `~/Library/Preferences/com.apple.SetupAssistant.plist`, plus
//!   `LastSeenCloudProductVersion` / `LastSeenBuddyBuildVersion` matching the OS
//!   (else the cloud screen re-prompts).
//!
//! This is a host-side reconciler over `hdiutil`/`diskutil`/`plutil`: attach the
//! guest disk read-write with no auto-mount, mount the synthesized container's
//! **Data** volume, write the markers, then unmount and detach robustly. A guard
//! detaches even if an edit fails part-way, so the image is never left attached.
//! It refuses to run if the image already appears attached (editing a mounted,
//! possibly running guest would corrupt the filesystem).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use snafu::{OptionExt, ResultExt, Snafu};

/// Per-user Setup Assistant `DidSee*` keys. Each false key shows its per-user
/// setup screen on first login; setting all true skips the whole per-user flow.
const DID_SEE_KEYS: &[&str] = &[
    "DidSeeScreenTime",
    "DidSeeSiriSetup",
    "DidSeeCloudSetup",
    "DidSeeAppearanceSetup",
    "DidSeeTouchIDSetup",
    "DidSeeApplePaySetup",
    "DidSeeSyncSetup",
    "DidSeeSyncSetup2",
    "DidSeeTermsOfAddress",
    "DidSeeActivationLock",
    "DidSeeLockdownMode",
    "DidSeeAppStore",
];

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("bundle {path:?} does not contain a disk.img"))]
    NoDisk { path: PathBuf },
    #[snafu(display(
        "disk image {path:?} is already attached; stop the VM (and detach the image) \
         before provisioning to avoid filesystem corruption"
    ))]
    ImageInUse { path: PathBuf },
    #[snafu(display("{tool} failed to run: {source}"))]
    Spawn {
        tool: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("{tool} exited with status {status}: {stderr}"))]
    Tool {
        tool: &'static str,
        status: String,
        stderr: String,
    },
    #[snafu(display("could not parse {tool} plist output: {source}"))]
    ParsePlist {
        tool: &'static str,
        source: serde_json::Error,
    },
    #[snafu(display(
        "no APFS Data volume found on the attached guest disk (devices: {devices:?})"
    ))]
    NoDataVolume { devices: Vec<String> },
    #[snafu(display("the Data volume mounted at an unexpected/empty path"))]
    NoMountPoint,
    #[snafu(display("could not read the guest OS version from {path:?}: {message}"))]
    GuestVersion { path: PathBuf, message: String },
    #[snafu(display("filesystem edit on the guest Data volume failed: {source}"))]
    Edit { source: std::io::Error },
    #[snafu(display("could not feed plist bytes to plutil: {source}"))]
    PipePlutil { source: std::io::Error },
    #[snafu(display("plutil produced no stdin pipe to write the plist to"))]
    PlutilNoStdin,
}

/// Parameters for [`provision`].
pub struct Provision {
    pub bundle: PathBuf,
    pub user: String,
    pub autologin: bool,
    pub password: String,
}

/// `diskutil apfs list -plist` result.
#[derive(Deserialize)]
struct ApfsList {
    #[serde(rename = "Containers")]
    containers: Vec<ApfsContainer>,
}

#[derive(Deserialize)]
struct ApfsContainer {
    #[serde(rename = "PhysicalStores")]
    physical_stores: Vec<ApfsPhysicalStore>,
    #[serde(rename = "Volumes")]
    volumes: Vec<ApfsVolume>,
}

#[derive(Deserialize)]
struct ApfsPhysicalStore {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
}

#[derive(Deserialize)]
struct ApfsVolume {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
    #[serde(rename = "Roles", default)]
    roles: Vec<String>,
}

/// `diskutil info -plist <dev>` result (the mount point after `diskutil mount`).
#[derive(Deserialize)]
struct DiskInfo {
    #[serde(rename = "MountPoint")]
    mount_point: Option<String>,
}

/// `hdiutil info -plist` result: the disk images currently attached.
#[derive(Deserialize)]
struct HdiutilInfo {
    #[serde(default)]
    images: Vec<HdiutilImage>,
}

#[derive(Deserialize)]
struct HdiutilImage {
    #[serde(rename = "image-path")]
    image_path: Option<String>,
}

/// Provision the stopped guest in `bundle`: mark Setup Assistant complete (and,
/// with `autologin`, enable password-less login) by editing its disk.
pub fn provision(params: Provision) -> Result<(), Error> {
    let Provision {
        bundle,
        user,
        autologin,
        password,
    } = params;

    let disk = bundle.join("disk.img");
    if !disk.exists() {
        return Err(Error::NoDisk { path: bundle });
    }
    // Refuse if the image is already attached: editing a live filesystem (a
    // running guest, or a stale attach) corrupts it.
    if image_attached(&disk)? {
        return Err(Error::ImageInUse { path: disk });
    }

    // Attach read-write with no auto-mount; APFS synthesizes the container as a
    // further /dev/disk we discover via `diskutil apfs list`.
    let attach_json = hdiutil_attach(&disk)?;
    // Build the teardown guard from the attached devices the instant the attach
    // succeeds, before any further parsing, so a parse failure here still
    // detaches the image rather than leaking it. The device scan is a lenient
    // substring pass over the raw plist-as-JSON that cannot itself fail.
    let devices = scan_dev_entries(&attach_json);
    let mut guard = Guard::new(devices.clone());

    let volumes = find_guest_volumes(&devices)?;
    let data_mount = mount_volume(&volumes.data)?;
    guard.add_mounted(volumes.data);

    // Read the guest OS version from its System volume. The Data volume does not
    // resolve the System firmlinks when mounted standalone from the host, so the
    // System volume must be mounted separately. Mount it read-only (we never
    // write the system volume) and from the SAME container as the Data volume.
    let system_mount = mount_volume_readonly(&volumes.system)?;
    guard.add_mounted(volumes.system);
    let version = guest_os_version(Path::new(&system_mount))?;

    // The edit is the only fallible-and-leaving-state step; the guard handles
    // unmount/detach regardless of how it ends.
    apply_edits(Path::new(&data_mount), &user, autologin, &password, &version)?;
    Ok(())
}

/// A RAII teardown: unmount every mounted volume and detach the attached disks,
/// robustly, when this drops. The synthesized container plus ISC/Recovery
/// sub-disks all hang off the physical disk, so `hdiutil detach` of each attached
/// device tears the tree down; we `force` to override a transient busy.
struct Guard {
    devices: Vec<String>,
    mounted: Vec<String>,
}

impl Guard {
    const fn new(devices: Vec<String>) -> Self {
        Self { devices, mounted: Vec::new() }
    }

    fn add_mounted(&mut self, dev: String) {
        self.mounted.push(dev);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        for dev in &self.mounted {
            // Best-effort unmount; the detach below is what actually frees the
            // image, but unmounting first avoids a "busy" detach.
            let _ = Command::new("/usr/sbin/diskutil")
                .args(["unmount", "force", dev])
                .output();
        }
        // Detach the attached disks. Sort so the base /dev/diskN (the attached
        // image) is detached; detaching it releases the synthesized container.
        // Dedupe: an `hdiutil attach` lists the whole-disk and its partitions,
        // but we only need to detach each whole disk once.
        let mut bases: Vec<&str> = self.devices.iter().map(|d| base_disk(d)).collect();
        bases.sort_unstable();
        bases.dedup();
        for base in bases {
            let _ = Command::new("/usr/bin/hdiutil")
                .args(["detach", base, "-force"])
                .output();
        }
    }
}

/// Whether `disk` already appears in `hdiutil info -plist` as an attached image.
/// Compares canonicalized paths so a symlinked or relative `disk` still matches
/// the absolute `image-path` hdiutil reports.
fn image_attached(disk: &Path) -> Result<bool, Error> {
    let json = run_plist_json("hdiutil", &["info", "-plist"])?;
    let info: HdiutilInfo =
        serde_json::from_str(&json).context(ParsePlistSnafu { tool: "hdiutil" })?;
    let want = std::fs::canonicalize(disk).unwrap_or_else(|_| disk.to_path_buf());
    Ok(info.images.iter().any(|img| {
        img.image_path.as_ref().is_some_and(|p| {
            let have = std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
            have == want
        })
    }))
}

/// Attach `disk` read-write with no mount and return the raw plist-as-JSON of
/// `hdiutil attach -plist`. The caller scans this for device identifiers with
/// [`scan_dev_entries`] and builds a teardown guard before any strict parse, so
/// a parse error never leaks the attachment.
fn hdiutil_attach(disk: &Path) -> Result<String, Error> {
    run_plist_json(
        "hdiutil",
        &["attach", "-plist", "-nomount", "-owners", "on", &disk.to_string_lossy()],
    )
}

/// Extract the attached `/dev/diskN[sM]` device identifiers (without the `/dev/`
/// prefix) from the raw attach output. This is a lenient substring scan rather
/// than a typed parse so it cannot fail after a successful attach: the guard
/// must be buildable from it even if the structured plist is unexpected. The
/// `plutil` JSON escapes `/` as `\/`, so match the device tokens directly.
fn scan_dev_entries(raw: &str) -> Vec<String> {
    // Devices appear as `/dev/diskN` or, JSON-escaped, `\/dev\/diskN`. Normalize
    // the escaping, then pull each `diskN[sM]` run after a `/dev/` marker.
    let normalized = raw.replace("\\/", "/");
    let mut devices: Vec<String> = Vec::new();
    for (idx, _) in normalized.match_indices("/dev/disk") {
        let rest = &normalized[idx + "/dev/".len()..];
        // A device id is `disk` followed by digits and optional `sN` partition
        // segments: take the leading run of ASCII alphanumerics.
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        let dev = &rest[..end];
        if dev.starts_with("disk") && dev.len() > "disk".len() {
            devices.push(dev.to_owned());
        }
    }
    devices.sort_unstable();
    devices.dedup();
    devices
}

/// The guest's Data and System volume device identifiers, both from the one
/// container backed by the attached image.
struct GuestVolumes {
    data: String,
    system: String,
}

/// Find the guest's Data and System volumes on the container backed by one of
/// `devices`. Both must come from the SAME container as the attached image, so a
/// `System` volume belonging to the host (which also appears in `diskutil apfs
/// list`) is never matched.
fn find_guest_volumes(devices: &[String]) -> Result<GuestVolumes, Error> {
    let json = run_plist_json("diskutil", &["apfs", "list", "-plist"])?;
    let list: ApfsList =
        serde_json::from_str(&json).context(ParsePlistSnafu { tool: "diskutil" })?;
    // Match a container whose physical store is one of our attached devices
    // (compare on the base disk: `disk5s1` belongs to attached `disk5`).
    let bases: Vec<&str> = devices.iter().map(|d| base_disk(d)).collect();
    for container in list.containers {
        let backed = container
            .physical_stores
            .iter()
            .any(|store| bases.contains(&base_disk(&store.device_identifier)));
        if !backed {
            continue;
        }
        let data = container
            .volumes
            .iter()
            .find(|v| v.roles.iter().any(|r| r == "Data"))
            .map(|v| v.device_identifier.clone());
        let system = container
            .volumes
            .iter()
            .find(|v| v.roles.iter().any(|r| r == "System"))
            .map(|v| v.device_identifier.clone());
        if let (Some(data), Some(system)) = (data, system) {
            return Ok(GuestVolumes { data, system });
        }
    }
    Err(Error::NoDataVolume { devices: devices.to_vec() })
}

/// The base disk identifier of a device (`disk5s1` -> `disk5`, `disk5` ->
/// `disk5`). The partition suffix is the `s<digits>` after the disk number, so
/// search past the literal `disk` prefix to avoid the `s` inside `disk` itself.
fn base_disk(dev: &str) -> &str {
    let after_prefix = dev.strip_prefix("disk").unwrap_or(dev);
    after_prefix
        .find('s')
        .map_or(dev, |rel| &dev[..dev.len() - (after_prefix.len() - rel)])
}

/// Mount a volume read-write and return its mount point. The caller records the
/// device on its [`Guard`] so teardown unmounts before detaching.
fn mount_volume(dev: &str) -> Result<String, Error> {
    mount_volume_with(dev, &["mount", dev])
}

/// Mount a volume read-only (for the System volume, which is only read).
fn mount_volume_readonly(dev: &str) -> Result<String, Error> {
    mount_volume_with(dev, &["mount", "readOnly", dev])
}

fn mount_volume_with(dev: &str, args: &[&str]) -> Result<String, Error> {
    run_checked("diskutil", args)?;
    let json = run_plist_json("diskutil", &["info", "-plist", dev])?;
    let info: DiskInfo =
        serde_json::from_str(&json).context(ParsePlistSnafu { tool: "diskutil" })?;
    let mount = info.mount_point.filter(|m| !m.is_empty()).context(NoMountPointSnafu)?;
    Ok(mount)
}

/// Write the Setup-Assistant-complete markers (and optional auto-login) onto the
/// mounted guest Data volume. `version` is the guest OS product version read
/// from its System volume.
fn apply_edits(
    data: &Path,
    user: &str,
    autologin: bool,
    password: &str,
    version: &str,
) -> Result<(), Error> {
    // 1. System SA: ensure `.AppleSetupDone` exists on the Data volume.
    let setup_done = data.join("private/var/db/.AppleSetupDone");
    if let Some(parent) = setup_done.parent() {
        std::fs::create_dir_all(parent).context(EditSnafu)?;
    }
    if !setup_done.exists() {
        std::fs::write(&setup_done, b"").context(EditSnafu)?;
    }

    // 2. Per-user SA: set every DidSee* key true in the user's
    //    com.apple.SetupAssistant.plist, creating it if absent.
    let prefs = data
        .join("Users")
        .join(user)
        .join("Library/Preferences/com.apple.SetupAssistant.plist");
    if let Some(parent) = prefs.parent() {
        std::fs::create_dir_all(parent).context(EditSnafu)?;
    }
    if !prefs.exists() {
        // `plutil -replace` needs a valid plist to edit; seed an empty dict.
        std::fs::write(&prefs, EMPTY_PLIST).context(EditSnafu)?;
    }
    for key in DID_SEE_KEYS {
        plutil_replace_bool(&prefs, key, true)?;
    }

    // 3. Match the cloud/buddy version keys to the guest OS so the cloud screen
    //    does not re-prompt. `version` was read from the guest's System volume by
    //    the caller (the System firmlinks do not resolve on a standalone-mounted
    //    Data volume, so it cannot be read from `data`).
    plutil_replace_string(&prefs, "LastSeenCloudProductVersion", version)?;
    plutil_replace_string(&prefs, "LastSeenBuddyBuildVersion", version)?;

    // 4. Optional auto-login.
    if autologin {
        enable_autologin(data, user, password)?;
    }
    Ok(())
}

/// Read the guest OS product version (e.g. `26.5`) from the System volume's
/// `SystemVersion.plist`. `system` is the System volume's mount point.
fn guest_os_version(system: &Path) -> Result<String, Error> {
    let plist = system.join("System/Library/CoreServices/SystemVersion.plist");
    if !plist.exists() {
        return Err(Error::GuestVersion {
            path: plist,
            message: "SystemVersion.plist not found on the guest System volume".to_owned(),
        });
    }
    let out = Command::new("/usr/bin/plutil")
        .args(["-extract", "ProductVersion", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .context(SpawnSnafu { tool: "plutil" })?;
    if !out.status.success() {
        return Err(Error::GuestVersion {
            path: plist,
            message: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if version.is_empty() {
        return Err(Error::GuestVersion {
            path: plist,
            message: "ProductVersion was empty".to_owned(),
        });
    }
    Ok(version)
}

/// Write `kcpassword` and the loginwindow `autoLoginUser` so the guest boots
/// straight to `user`'s desktop with no password typing.
fn enable_autologin(data: &Path, user: &str, password: &str) -> Result<(), Error> {
    // /etc/kcpassword holds the obfuscated auto-login password (XOR with a fixed
    // key, NUL-padded to the next 12-byte block). It lives on the Data volume's
    // firmlinked /etc (-> /private/etc).
    let kcpassword = data.join("private/etc/kcpassword");
    std::fs::write(&kcpassword, encode_kcpassword(password)).context(EditSnafu)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // root-only, matching the real file's 0600.
        let _ = std::fs::set_permissions(&kcpassword, std::fs::Permissions::from_mode(0o600));
    }

    // The loginwindow preference selecting the auto-login user.
    let loginwindow = data.join("Library/Preferences/com.apple.loginwindow.plist");
    if let Some(parent) = loginwindow.parent() {
        std::fs::create_dir_all(parent).context(EditSnafu)?;
    }
    if !loginwindow.exists() {
        std::fs::write(&loginwindow, EMPTY_PLIST).context(EditSnafu)?;
    }
    plutil_replace_string(&loginwindow, "autoLoginUser", user)?;
    Ok(())
}

/// Encode an auto-login password the way macOS's `kcpassword` expects: XOR each
/// byte with the fixed cipher key (cycling), padding with NULs to a 12-byte
/// boundary. Matching Apple's loginwindow, when the password length is already a
/// multiple of 12 (including the empty password) a whole extra 12-byte block is
/// appended, so there is always at least one trailing pad byte.
fn encode_kcpassword(password: &str) -> Vec<u8> {
    const CIPHER: [u8; 11] = [0x7d, 0x89, 0x52, 0x23, 0xd2, 0xbc, 0xdd, 0xea, 0xa3, 0xb9, 0x1f];
    const BLOCK: usize = 12;
    let bytes = password.as_bytes();
    // Round up to the next block, then if the length already sits exactly on a
    // block boundary add one more full block (Apple appends a trailing pad block
    // in that case so the encrypted form is never an un-padded exact multiple).
    let mut padded_len = bytes.len().div_ceil(BLOCK) * BLOCK;
    if bytes.len().is_multiple_of(BLOCK) {
        padded_len += BLOCK;
    }
    let mut out = Vec::with_capacity(padded_len);
    for i in 0..padded_len {
        let plain = bytes.get(i).copied().unwrap_or(0);
        out.push(plain ^ CIPHER[i % CIPHER.len()]);
    }
    out
}

/// `plutil -replace <key> -bool <value>` on `plist`.
fn plutil_replace_bool(plist: &Path, key: &str, value: bool) -> Result<(), Error> {
    run_checked(
        "plutil",
        &[
            "-replace",
            key,
            "-bool",
            if value { "true" } else { "false" },
            &plist.to_string_lossy(),
        ],
    )
}

/// `plutil -replace <key> -string <value>` on `plist`.
fn plutil_replace_string(plist: &Path, key: &str, value: &str) -> Result<(), Error> {
    run_checked(
        "plutil",
        &["-replace", key, "-string", value, &plist.to_string_lossy()],
    )
}

/// An empty binary plist (a dict with no keys), the seed for a fresh
/// preferences file `plutil -replace` can then edit.
const EMPTY_PLIST: &[u8] =
    b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
      <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
      \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
      <plist version=\"1.0\"><dict/></plist>\n";

/// Run a tool that emits a plist on stdout, converting it to JSON with `plutil`
/// so it parses with `serde_json`. Returns the JSON text.
fn run_plist_json(tool: &'static str, args: &[&str]) -> Result<String, Error> {
    let tool_path = tool_path(tool);
    let out = Command::new(tool_path)
        .args(args)
        .output()
        .context(SpawnSnafu { tool })?;
    if !out.status.success() {
        return Err(Error::Tool {
            tool,
            status: out.status.to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    // Pipe the plist bytes through `plutil -convert json -o - -`.
    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context(SpawnSnafu { tool: "plutil" })?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().context(PlutilNoStdinSnafu)?;
        stdin.write_all(&out.stdout).context(PipePlutilSnafu)?;
    }
    let converted = child.wait_with_output().context(SpawnSnafu { tool: "plutil" })?;
    if !converted.status.success() {
        return Err(Error::Tool {
            tool: "plutil",
            status: converted.status.to_string(),
            stderr: String::from_utf8_lossy(&converted.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&converted.stdout).into_owned())
}

/// Run a tool, mapping a spawn failure or non-zero exit to a typed error.
fn run_checked(tool: &'static str, args: &[&str]) -> Result<(), Error> {
    let out = Command::new(tool_path(tool))
        .args(args)
        .output()
        .context(SpawnSnafu { tool })?;
    if out.status.success() {
        return Ok(());
    }
    Err(Error::Tool {
        tool,
        status: out.status.to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Absolute path for a system tool (`diskutil` is under `/usr/sbin`, the rest
/// under `/usr/bin`), so the reconciler does not depend on the caller's `PATH`.
fn tool_path(tool: &str) -> PathBuf {
    match tool {
        "diskutil" => PathBuf::from("/usr/sbin/diskutil"),
        other => PathBuf::from("/usr/bin").join(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_disk_strips_partition() {
        assert_eq!(base_disk("disk5"), "disk5");
        assert_eq!(base_disk("disk5s1"), "disk5");
        assert_eq!(base_disk("disk12s3"), "disk12");
    }

    #[test]
    fn kcpassword_empty_is_one_padded_block() {
        // Empty password (length 0, a multiple of 12): one full 12-byte pad
        // block, per Apple's append-a-block-on-exact-multiple rule.
        let enc = encode_kcpassword("");
        assert_eq!(enc.len(), 12);
    }

    #[test]
    fn kcpassword_roundtrips_via_xor() {
        // XORing the encoding back with the cipher recovers the plaintext bytes
        // (the rest is NUL padding).
        const CIPHER: [u8; 11] =
            [0x7d, 0x89, 0x52, 0x23, 0xd2, 0xbc, 0xdd, 0xea, 0xa3, 0xb9, 0x1f];
        let pw = "hunter2";
        let enc = encode_kcpassword(pw);
        let decoded: Vec<u8> = enc
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ CIPHER[i % CIPHER.len()])
            .collect();
        assert_eq!(&decoded[..pw.len()], pw.as_bytes());
        assert!(decoded[pw.len()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn kcpassword_block_size_rounds_up() {
        // A 13-char password needs two 12-byte blocks.
        assert_eq!(encode_kcpassword("aaaaaaaaaaaaa").len(), 24);
    }

    #[test]
    fn kcpassword_exact_multiple_appends_a_block() {
        // A length that is already a multiple of 12 gets a whole extra pad block
        // (12 -> 24, 24 -> 36), matching Apple's loginwindow.
        assert_eq!(encode_kcpassword(&"a".repeat(12)).len(), 24);
        assert_eq!(encode_kcpassword(&"a".repeat(24)).len(), 36);
        assert_eq!(encode_kcpassword(&"a".repeat(11)).len(), 12);
    }

    #[test]
    fn scan_dev_entries_handles_escaped_and_plain() {
        // The plutil JSON escapes `/` as `\/`; both forms must yield the base
        // and partition devices, deduped and sorted.
        let escaped = r#"{"system-entities":[{"dev-entry":"\/dev\/disk7"},{"dev-entry":"\/dev\/disk7s1"}]}"#;
        assert_eq!(scan_dev_entries(escaped), vec!["disk7", "disk7s1"]);
        let plain = r#"{"dev-entry":"/dev/disk4","x":"/dev/disk4s2"}"#;
        assert_eq!(scan_dev_entries(plain), vec!["disk4", "disk4s2"]);
        // No devices -> empty (e.g. a parse-failure-shaped blob).
        assert!(scan_dev_entries("{}").is_empty());
    }
}
