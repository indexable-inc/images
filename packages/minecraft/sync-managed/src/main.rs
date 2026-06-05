use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::Parser;
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(about = "Sync ix-managed Minecraft files into the mutable data directory")]
struct Config {
    #[arg(long)]
    data_dir: PathBuf,

    #[arg(long)]
    dropin_dir: String,

    #[arg(long)]
    managed_root: PathBuf,

    #[arg(long)]
    plugman_reload: bool,

    #[arg(long)]
    rcon_enable: bool,

    #[arg(long = "plugman-ignored-plugin")]
    plugman_ignored_plugins: Vec<String>,

    #[arg(long = "datapack-world")]
    datapack_worlds: Vec<String>,

    #[arg(long)]
    rcon_port: u16,

    #[arg(long)]
    rcon_password_file: PathBuf,

    #[arg(long, action = clap::ArgAction::Set, value_parser = parse_bool)]
    rcon_broadcast_to_ops: bool,
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("expected true or false".to_owned()),
    }
}

fn managed_files(source_dir: &Path) -> Result<Vec<String>> {
    if !source_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    collect_managed_files(source_dir, source_dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn validate_rel_path_shape(rel: &str) -> Result<()> {
    ensure!(!rel.is_empty(), "path is empty");

    ensure!(!Path::new(rel).is_absolute(), "path is absolute: {rel}");

    if rel
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        bail!("path contains unsafe segment: {rel}");
    }

    Ok(())
}

fn validate_rel_path(rel: &str) -> Result<()> {
    validate_rel_path_shape(rel)?;

    if !rel
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '+' | '-'))
    {
        bail!("path contains shell-sensitive or unsafe characters: {rel}");
    }

    Ok(())
}

fn collect_managed_files(root: &Path, dir: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading file type for {}", path.display()))?;

        if file_type.is_dir() {
            collect_managed_files(root, &path, files)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            let rel = path
                .strip_prefix(root)
                .with_context(|| {
                    format!("making {} relative to {}", path.display(), root.display())
                })?
                .to_string_lossy()
                .replace('\\', "/");

            if !rel.ends_with(".plugin-name") {
                files.push(rel);
            }
        }
    }

    Ok(())
}

/// A parsed manifest line: the relative path and an optional absolute source target.
#[derive(Debug, PartialEq, Eq)]
struct ManifestEntry<'a> {
    rel: &'a str,
    target: Option<&'a str>,
}

fn manifest_entry(line: &str) -> ManifestEntry<'_> {
    match line.rsplit_once(' ') {
        Some((rel, target)) if Path::new(target).is_absolute() => ManifestEntry {
            rel,
            target: Some(target),
        },
        _ => ManifestEntry {
            rel: line,
            target: None,
        },
    }
}

fn read_manifest_lines(manifest: &Path) -> Result<Vec<String>> {
    match fs::read_to_string(manifest) {
        Ok(text) => Ok(text.lines().map(str::to_owned).collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", manifest.display())),
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

fn sync_tree(
    source_dir: &Path,
    target_dir: &Path,
    manifest: &Path,
    preserve_removed: &BTreeSet<String>,
) -> Result<()> {
    fs::create_dir_all(target_dir).with_context(|| format!("creating {}", target_dir.display()))?;
    if let Some(parent) = manifest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    for (i, line) in read_manifest_lines(manifest)?.into_iter().enumerate() {
        let ManifestEntry { rel, .. } = manifest_entry(&line);
        if rel.is_empty() || preserve_removed.contains(rel) {
            continue;
        }
        validate_rel_path_shape(rel).with_context(|| {
            format!(
                "checking manifest path safety in {} at line {}",
                manifest.display(),
                i + 1
            )
        })?;
        remove_if_present(&target_dir.join(rel))?;
    }

    let tmp = manifest.with_file_name(format!(
        "{}.tmp",
        manifest
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!(
                "manifest path has no UTF-8 file name: {}",
                manifest.display()
            ))?
    ));

    let mut manifest_lines = String::new();
    for rel in managed_files(source_dir)? {
        validate_rel_path(&rel).with_context(|| {
            format!(
                "checking managed file path safety in {}",
                source_dir.display()
            )
        })?;
        let source_path = source_dir.join(&rel);
        let target_path = target_dir.join(&rel);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        remove_if_present(&target_path)?;
        symlink(&source_path, &target_path).with_context(|| {
            format!(
                "linking {} to {}",
                target_path.display(),
                source_path.display()
            )
        })?;
        writeln!(
            &mut manifest_lines,
            "{} {}",
            rel,
            source_path.canonicalize()?.display()
        )
        .expect("writing to String cannot fail");
    }

    fs::write(&tmp, manifest_lines).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, manifest).with_context(|| format!("replacing {}", manifest.display()))?;
    Ok(())
}

fn managed_target_for(manifest: &Path, rel: &str) -> Result<Option<String>> {
    for line in read_manifest_lines(manifest)? {
        let ManifestEntry { rel: entry_rel, target } = manifest_entry(&line);
        if entry_rel == rel
            && let Some(target) = target
        {
            return Ok(Some(target.to_owned()));
        }
    }
    Ok(None)
}

fn plugin_name_for(managed_root: &Path, rel: &str) -> Result<String> {
    let metadata = managed_root
        .join("managed-dropins")
        .join(format!("{rel}.plugin-name"));
    if metadata.exists() {
        let text = fs::read_to_string(&metadata)
            .with_context(|| format!("reading {}", metadata.display()))?;
        if let Some(line) = text.lines().next()
            && !line.is_empty()
        {
            return Ok(line.to_owned());
        }
    }

    Ok(Path::new(rel)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| anyhow!("cannot infer plugin name from {rel}"))?
        .to_owned())
}

fn plugin_name_from_config_path(rel: &str) -> Option<&str> {
    let mut parts = rel.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("plugins"), Some(plugin), Some(_)) => Some(plugin),
        _ => None,
    }
}

/// `PlugManX` lifecycle verb the reload plan asks the server to perform for one
/// plugin. Variant order is the plan's sort order, kept identical to the legacy
/// stringly plan (`load` < `reload` < `unload`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReloadAction {
    Load,
    Reload,
    Unload,
}

impl ReloadAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Load => "load",
            Self::Reload => "reload",
            Self::Unload => "unload",
        }
    }
}

/// One line of the reload plan: a lifecycle verb paired with the plugin it
/// applies to. Ordering matches the rendered plan file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReloadEntry {
    action: ReloadAction,
    plugin: String,
}

fn write_plan(plan_path: &Path, plan: &BTreeSet<ReloadEntry>) -> Result<()> {
    let mut lines = String::new();
    for ReloadEntry { action, plugin } in plan {
        writeln!(&mut lines, "{} {plugin}", action.as_str()).expect("writing to String cannot fail");
    }

    fs::write(plan_path, lines).with_context(|| format!("writing {}", plan_path.display()))
}

fn is_jar_path(rel: &str) -> bool {
    Path::new(rel)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
}

fn plan_dropin_reloads(config: &Config, plan: &mut BTreeSet<ReloadEntry>) -> Result<()> {
    let dropin_manifest = config
        .data_dir
        .join(format!(".ix-managed-{}", config.dropin_dir));
    let managed_dropins = config.managed_root.join("managed-dropins");
    if !(dropin_manifest.exists() && managed_dropins.exists()) {
        return Ok(());
    }

    let ignored: BTreeSet<_> = config.plugman_ignored_plugins.iter().cloned().collect();
    for rel in managed_files(&managed_dropins)? {
        if rel == "PlugManX.jar" || !is_jar_path(&rel) {
            continue;
        }

        let target = managed_dropins
            .join(&rel)
            .canonicalize()?
            .display()
            .to_string();
        let old_target = managed_target_for(&dropin_manifest, &rel)?;
        let plugin = plugin_name_for(&config.managed_root, &rel)?;
        if ignored.contains(&plugin) {
            continue;
        }

        match old_target {
            None => {
                plan.insert(ReloadEntry {
                    action: ReloadAction::Load,
                    plugin,
                });
            }
            Some(old_target) if old_target != target => {
                plan.insert(ReloadEntry {
                    action: ReloadAction::Reload,
                    plugin,
                });
            }
            Some(_) => {}
        }
    }

    for line in read_manifest_lines(&dropin_manifest)? {
        let ManifestEntry { rel, .. } = manifest_entry(&line);
        if !is_jar_path(rel) || rel == "PlugManX.jar" {
            continue;
        }

        let plugin = plugin_name_for(&config.managed_root, rel)?;
        if ignored.contains(&plugin) {
            continue;
        }
        if !managed_dropins.join(rel).exists() {
            plan.insert(ReloadEntry {
                action: ReloadAction::Unload,
                plugin,
            });
        }
    }

    Ok(())
}

fn plan_server_file_reloads(config: &Config, plan: &mut BTreeSet<ReloadEntry>) -> Result<()> {
    let server_manifest = config.data_dir.join(".ix-managed-server-files");
    let managed_server_files = config.managed_root.join("managed-server-files");
    if !(server_manifest.exists() && managed_server_files.exists()) {
        return Ok(());
    }

    let ignored: BTreeSet<_> = config.plugman_ignored_plugins.iter().cloned().collect();
    for rel in managed_files(&managed_server_files)? {
        let Some(plugin) = plugin_name_from_config_path(&rel) else {
            continue;
        };
        if ignored.contains(plugin) {
            continue;
        }

        let target = managed_server_files
            .join(&rel)
            .canonicalize()?
            .display()
            .to_string();
        let old_target = managed_target_for(&server_manifest, &rel)?;
        if old_target.as_deref() != Some(target.as_str()) {
            plan.insert(ReloadEntry {
                action: ReloadAction::Reload,
                plugin: plugin.to_owned(),
            });
        }
    }

    for line in read_manifest_lines(&server_manifest)? {
        let ManifestEntry { rel, .. } = manifest_entry(&line);
        let Some(plugin) = plugin_name_from_config_path(rel) else {
            continue;
        };
        if ignored.contains(plugin) {
            continue;
        }
        if !managed_server_files.join(rel).exists() {
            plan.insert(ReloadEntry {
                action: ReloadAction::Reload,
                plugin: plugin.to_owned(),
            });
        }
    }

    Ok(())
}

fn plan_plugman_reload(config: &Config) -> Result<()> {
    let plan_path = config
        .data_dir
        .join(format!(".ix-managed-{}.reload-plan", config.dropin_dir));
    if let Some(parent) = plan_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut plan = BTreeSet::new();
    plan_dropin_reloads(config, &mut plan)?;
    plan_server_file_reloads(config, &mut plan)?;
    write_plan(&plan_path, &plan)
}

fn ensure_rcon_password(config: &Config) -> Result<()> {
    if let Some(parent) = config.rcon_password_file.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    if config.rcon_password_file.exists()
        && !fs::read_to_string(&config.rcon_password_file)
            .with_context(|| format!("reading {}", config.rcon_password_file.display()))?
            .trim()
            .is_empty()
    {
        return Ok(());
    }

    let password = generate_password()?;
    fs::write(&config.rcon_password_file, format!("{password}\n"))
        .with_context(|| format!("writing {}", config.rcon_password_file.display()))?;
    set_owner_read_write(&config.rcon_password_file)
}

fn generate_password() -> Result<String> {
    let mut password = String::new();
    for _ in 0..2 {
        password.push_str(&read_kernel_uuid()?.replace('-', ""));
    }
    Ok(password)
}

fn read_kernel_uuid() -> Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/uuid")?
        .trim()
        .to_owned())
}

fn set_owner_read_write(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let permissions = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("chmod 0600 {}", path.display()))
    }

    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)
            .with_context(|| format!("reading metadata for {}", path.display()))?
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("making {} owner writable", path.display()))
    }
}

fn set_property(file: &Path, key: &str, value: &str) -> Result<()> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", file.display())),
    };

    let replacement = format!("{key}={value}");
    let mut found = false;
    let mut next_lines = Vec::new();
    for line in text.lines() {
        if line.starts_with(&format!("{key}=")) {
            next_lines.push(replacement.clone());
            found = true;
        } else {
            next_lines.push(line.to_owned());
        }
    }

    if !found {
        next_lines.push(replacement);
    }

    fs::write(file, format!("{}\n", next_lines.join("\n")))
        .with_context(|| format!("writing {}", file.display()))
}

fn configure_rcon(config: &Config) -> Result<()> {
    ensure_rcon_password(config)?;
    let server_properties = config.data_dir.join("server.properties");

    if server_properties.is_symlink() {
        let tmp = server_properties.with_file_name("server.properties.tmp");
        fs::copy(&server_properties, &tmp).with_context(|| {
            format!(
                "copying {} to {}",
                server_properties.display(),
                tmp.display()
            )
        })?;
        fs::rename(&tmp, &server_properties)
            .with_context(|| format!("replacing {}", server_properties.display()))?;
    } else if !server_properties.exists() {
        fs::write(&server_properties, "")
            .with_context(|| format!("creating {}", server_properties.display()))?;
    }

    set_owner_read_write(&server_properties)?;
    let password = fs::read_to_string(&config.rcon_password_file)
        .with_context(|| format!("reading {}", config.rcon_password_file.display()))?
        .lines()
        .next()
        .ok_or_else(|| anyhow!("{} is empty", config.rcon_password_file.display()))?
        .to_owned();

    set_property(&server_properties, "enable-rcon", "true")?;
    set_property(
        &server_properties,
        "rcon.port",
        &config.rcon_port.to_string(),
    )?;
    set_property(&server_properties, "rcon.password", &password)?;
    set_property(
        &server_properties,
        "broadcast-rcon-to-ops",
        if config.rcon_broadcast_to_ops {
            "true"
        } else {
            "false"
        },
    )
}

fn read_json_entries(path: &Path) -> Result<Vec<Value>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };

    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing JSON from {}", path.display()))?;
    match value {
        Value::Array(entries) => {
            for entry in &entries {
                if !entry.is_object() {
                    bail!("{} contains a non-object entry", path.display());
                }
            }
            Ok(entries)
        }
        _ => bail!("{} must contain a JSON array", path.display()),
    }
}

fn entry_uuid(entry: &Value) -> Option<&str> {
    entry
        .get("uuid")
        .and_then(Value::as_str)
        .filter(|uuid| !uuid.is_empty())
}

/// Reject access files whose entries lack UUIDs or repeat one. The reconcile
/// step keys on UUID, so a missing or duplicate UUID would silently drop or
/// merge players; fail loudly before that can happen.
fn validate_unique_uuids(path: &Path, entries: &[Value]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for entry in entries {
        let uuid = entry_uuid(entry)
            .ok_or_else(|| anyhow!("{} contains an entry without a UUID", path.display()))?;
        if !seen.insert(uuid.to_owned()) {
            bail!("{} contains duplicate UUID {}", path.display(), uuid);
        }
    }
    Ok(())
}

fn reconcile_entries(current: &[Value], previous: &[Value], desired: &[Value]) -> Vec<Value> {
    let previous_uuids: BTreeSet<_> = previous
        .iter()
        .filter_map(entry_uuid)
        .map(str::to_owned)
        .collect();
    let desired_by_uuid: BTreeMap<_, _> = desired
        .iter()
        .filter_map(|entry| entry_uuid(entry).map(|uuid| (uuid.to_owned(), entry.clone())))
        .collect();
    let mut emitted = BTreeSet::new();
    let mut next_entries = Vec::new();

    for entry in current {
        let Some(uuid) = entry_uuid(entry) else {
            next_entries.push(entry.clone());
            continue;
        };

        if let Some(desired_entry) = desired_by_uuid.get(uuid) {
            next_entries.push(desired_entry.clone());
            emitted.insert(uuid.to_owned());
        } else if !previous_uuids.contains(uuid) {
            next_entries.push(entry.clone());
        }
    }

    for entry in desired {
        if let Some(uuid) = entry_uuid(entry)
            && !emitted.contains(uuid)
        {
            next_entries.push(entry.clone());
        }
    }

    next_entries
}

fn write_json_entries(path: &Path, entries: &[Value]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("path has no UTF-8 file name: {}", path.display()))?
    ));
    let json = serde_json::to_string_pretty(entries)?;
    fs::write(&tmp, format!("{json}\n")).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn old_server_file_entries(config: &Config, name: &str) -> Result<Vec<Value>> {
    let manifest = config.data_dir.join(".ix-managed-server-files");
    if managed_target_for(&manifest, name)?.is_none() {
        return Ok(Vec::new());
    }

    read_json_entries(&config.data_dir.join(name))
}

fn reconcile_access_file(config: &Config, name: &str) -> Result<()> {
    let desired_path = config.managed_root.join("managed-access").join(name);
    if !desired_path.exists() {
        return Ok(());
    }

    let live_path = config.data_dir.join(name);
    let state_path = config.data_dir.join(".ix-managed-access").join(name);

    let desired = read_json_entries(&desired_path)?;
    validate_unique_uuids(&desired_path, &desired)?;
    let current = read_json_entries(&live_path)?;
    let previous = if state_path.exists() {
        read_json_entries(&state_path)?
    } else {
        old_server_file_entries(config, name)?
    };
    validate_unique_uuids(&state_path, &previous)?;
    validate_unique_uuids(&live_path, &current)?;
    let next_entries = reconcile_entries(&current, &previous, &desired);

    write_json_entries(&live_path, &next_entries)?;
    write_json_entries(&state_path, &desired)
}

fn reconcile_access(config: &Config) -> Result<()> {
    reconcile_access_file(config, "whitelist.json")?;
    reconcile_access_file(config, "ops.json")
}

fn main() -> Result<()> {
    let config = Config::parse();

    if config.plugman_reload {
        plan_plugman_reload(&config)?;
    }

    sync_tree(
        &config.managed_root.join("managed-dropins"),
        &config.data_dir.join(&config.dropin_dir),
        &config
            .data_dir
            .join(format!(".ix-managed-{}", config.dropin_dir)),
        &BTreeSet::new(),
    )?;
    sync_tree(
        &config.managed_root.join("managed-config"),
        &config.data_dir.join("config"),
        &config.data_dir.join(".ix-managed-config"),
        &BTreeSet::new(),
    )?;
    sync_tree(
        &config.managed_root.join("managed-server-files"),
        &config.data_dir,
        &config.data_dir.join(".ix-managed-server-files"),
        &BTreeSet::from(["ops.json".to_owned(), "whitelist.json".to_owned()]),
    )?;
    for world in &config.datapack_worlds {
        validate_rel_path_shape(world).context("checking datapack world name safety")?;
        sync_tree(
            &config.managed_root.join("managed-datapacks"),
            &config.data_dir.join(world).join("datapacks"),
            &config.data_dir.join(world).join(".ix-managed-datapacks"),
            &BTreeSet::new(),
        )?;
    }
    reconcile_access(&config)?;

    if config.rcon_enable {
        configure_rcon(&config)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn entry(uuid: &str, name: &str) -> Value {
        Value::Object(Map::from_iter([
            ("uuid".to_owned(), Value::String(uuid.to_owned())),
            ("name".to_owned(), Value::String(name.to_owned())),
        ]))
    }

    #[test]
    fn validate_rel_path_rejects_unsafe_paths() {
        assert!(validate_rel_path("safe/path.json").is_ok());
        assert!(validate_rel_path("config.toml").is_ok());
        assert!(validate_rel_path("mods/pack+v1..2.toml").is_ok());

        assert!(validate_rel_path("My World").is_err());
        assert!(validate_rel_path("../escape.json").is_err());
        assert!(validate_rel_path("path/../escape.json").is_err());
        assert!(validate_rel_path("./escape.json").is_err());
        assert!(validate_rel_path("path/./escape.json").is_err());
        assert!(validate_rel_path("path//escape.json").is_err());
        assert!(validate_rel_path("path/escape.json/").is_err());
        assert!(validate_rel_path("/absolute/path").is_err());
        assert!(validate_rel_path("path with space.json").is_err());
        assert!(validate_rel_path("path;with;shell").is_err());
        assert!(validate_rel_path("path/with$(cmd)").is_err());
        assert!(validate_rel_path("path/with`cmd`").is_err());
        assert!(validate_rel_path("path/with|pipe").is_err());
        assert!(validate_rel_path("path/with\\backslash").is_err());
        assert!(validate_rel_path("").is_err());
    }

    #[test]
    fn validate_rel_path_shape_allows_legacy_manifest_cleanup_paths() {
        assert!(validate_rel_path_shape("plugins/Foo;Bar.yml").is_ok());
        assert!(validate_rel_path_shape("plugins/with space.yml").is_ok());
        assert!(validate_rel_path_shape("My World").is_ok());

        assert!(validate_rel_path_shape("../escape.json").is_err());
        assert!(validate_rel_path_shape("path/../escape.json").is_err());
        assert!(validate_rel_path_shape("path//escape.json").is_err());
        assert!(validate_rel_path_shape("/absolute/path").is_err());
    }

    #[test]
    fn manifest_entry_keeps_legacy_paths_with_spaces() {
        assert_eq!(
            manifest_entry("plugins/Foo Bar.yml /nix/store/source"),
            ManifestEntry {
                rel: "plugins/Foo Bar.yml",
                target: Some("/nix/store/source"),
            }
        );
        assert_eq!(
            manifest_entry("plugins/Foo;Bar.yml"),
            ManifestEntry {
                rel: "plugins/Foo;Bar.yml",
                target: None,
            }
        );
        assert_eq!(
            manifest_entry("plugins/Foo Bar.yml"),
            ManifestEntry {
                rel: "plugins/Foo Bar.yml",
                target: None,
            }
        );
    }

    #[test]
    fn reconcile_updates_managed_entries_and_preserves_manual_entries() {
        let current = vec![
            entry("managed-kept", "Old name"),
            entry("manual", "Manual"),
            entry("managed-removed", "Removed"),
        ];
        let previous = vec![
            entry("managed-kept", "Old name"),
            entry("managed-removed", "Removed"),
        ];
        let desired = vec![
            entry("managed-kept", "New name"),
            entry("managed-added", "Added"),
        ];

        let next = reconcile_entries(&current, &previous, &desired);

        assert_eq!(
            next,
            vec![
                entry("managed-kept", "New name"),
                entry("manual", "Manual"),
                entry("managed-added", "Added"),
            ]
        );
    }
}
