use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, btree_map::Entry};
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use tempfile::tempdir;

const DEFAULT_MIN_EFFICIENCY: f64 = 0.95;
const DEFAULT_MAX_WASTED_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_MAX_WASTED_PERCENT: f64 = 0.20;
const DEFAULT_EFFICIENCY_TOP_PATHS: usize = 10;

struct Args {
    conf_path: PathBuf,
    out_path: PathBuf,
    efficiency_policy: Option<EfficiencyPolicy>,
}

#[derive(Clone, Copy, Debug)]
struct EfficiencyPolicy {
    min_efficiency: f64,
    max_wasted_bytes: u64,
    max_wasted_percent: f64,
    top_paths: usize,
}

impl Default for EfficiencyPolicy {
    fn default() -> Self {
        Self {
            min_efficiency: DEFAULT_MIN_EFFICIENCY,
            max_wasted_bytes: DEFAULT_MAX_WASTED_BYTES,
            max_wasted_percent: DEFAULT_MAX_WASTED_PERCENT,
            top_paths: DEFAULT_EFFICIENCY_TOP_PATHS,
        }
    }
}

#[derive(Deserialize)]
struct Config {
    architecture: String,
    #[serde(rename = "config")]
    settings: Value,
    from_image: Value,
    store_layers: Vec<Vec<String>>,
    customisation_layer: String,
    created: String,
    mtime: String,
    uid: String,
    gid: String,
    store_dir: String,
}

struct Layer {
    checksum: String,
    size: u64,
    paths: Vec<String>,
    tar_path: PathBuf,
}

/// The result of streaming bytes through a hasher: the total byte count written
/// and the lowercase-hex sha256 digest of those bytes.
struct HashedBytes {
    size: u64,
    checksum: String,
}

#[derive(Debug, PartialEq)]
struct LayerEfficiency {
    entries: usize,
    paths: usize,
    repeated_paths: usize,
    discovered_bytes: u64,
    required_bytes: u64,
    wasted_bytes: u64,
    efficiency: f64,
    wasted_percent: f64,
    inefficient_paths: Vec<InefficientPath>,
}

#[derive(Debug, PartialEq)]
struct InefficientPath {
    path: String,
    occurrences: usize,
    cumulative_size: u64,
    required_size: u64,
}

impl InefficientPath {
    const fn wasted_bytes(&self) -> u64 {
        self.cumulative_size.saturating_sub(self.required_size)
    }
}

struct PathStats {
    occurrences: usize,
    cumulative_size: u64,
    required_size: u64,
    last_layer: usize,
}

enum Whiteout {
    Remove(String),
    Opaque(String),
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(env::args())?;

    let conf: Config = serde_json::from_reader(File::open(&args.conf_path)?)?;

    if !conf.from_image.is_null() {
        return Err("oci-image-builder: fromImage is not supported".into());
    }

    let created = parse_time(&conf.created)?.to_rfc3339_opts(SecondsFormat::Secs, false);
    let mtime = parse_time(&conf.mtime)?.timestamp().to_string();
    let work = tempdir()?;
    let image_dir = work.path().join("image");
    let blobs_dir = image_dir.join("blobs/sha256");
    let layers_dir = work.path().join("layers");
    fs::create_dir_all(&blobs_dir)?;
    fs::create_dir_all(&layers_dir)?;
    fs::write(
        image_dir.join("oci-layout"),
        r#"{"imageLayoutVersion":"1.0.0"}"#,
    )?;

    eprintln!("No 'fromImage' provided");

    let mut layers = Vec::with_capacity(conf.store_layers.len() + 1);
    for (index, paths) in conf.store_layers.iter().enumerate() {
        layers.push(make_store_layer(
            index + 1,
            paths,
            &conf,
            &mtime,
            &layers_dir,
            &blobs_dir,
        )?);
    }

    layers.push(make_customisation_layer(
        conf.store_layers.len() + 1,
        &conf.customisation_layer,
        &blobs_dir,
    )?);

    if let Some(policy) = args.efficiency_policy {
        let efficiency = analyze_layer_efficiency(&layers)?;
        report_layer_efficiency(&efficiency, policy)?;
    }

    eprintln!("Adding manifests...");
    write_metadata(&conf, &created, &layers, &image_dir, &mtime, &args.out_path)?;
    eprintln!("Done.");

    Ok(())
}

fn parse_args<I>(args: I) -> Result<Args, Box<dyn Error>>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let program = args
        .first()
        .map_or_else(|| "oci-image-builder".to_owned(), String::to_owned);
    let mut iter = args.into_iter().skip(1);
    let mut efficiency_policy = EfficiencyPolicy::default();
    let mut efficiency_enabled = true;
    let mut positional = Vec::new();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--skip-efficiency-check" => {
                efficiency_enabled = false;
            }
            "--min-efficiency" => {
                let value = next_arg(&mut iter, "--min-efficiency")?;
                efficiency_policy.min_efficiency = parse_ratio(&value, "--min-efficiency")?;
            }
            "--max-wasted-bytes" => {
                let value = next_arg(&mut iter, "--max-wasted-bytes")?;
                efficiency_policy.max_wasted_bytes =
                    parse_byte_limit(&value, "--max-wasted-bytes")?;
            }
            "--max-wasted-percent" => {
                let value = next_arg(&mut iter, "--max-wasted-percent")?;
                efficiency_policy.max_wasted_percent = parse_ratio(&value, "--max-wasted-percent")?;
            }
            "--efficiency-top-paths" => {
                let value = next_arg(&mut iter, "--efficiency-top-paths")?;
                efficiency_policy.top_paths = value.parse()?;
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown argument: {arg}").into());
            }
            _ => {
                positional.push(PathBuf::from(arg));
            }
        }
    }

    if positional.len() != 2 {
        return Err(format!(
            "usage: {program} [--skip-efficiency-check] [--min-efficiency <ratio>] [--max-wasted-bytes <bytes>] [--max-wasted-percent <ratio>] [--efficiency-top-paths <count>] <conf.json> <out.tar>"
        )
        .into());
    }

    Ok(Args {
        conf_path: positional.remove(0),
        out_path: positional.remove(0),
        efficiency_policy: efficiency_enabled.then_some(efficiency_policy),
    })
}

fn next_arg<I>(args: &mut I, flag: &str) -> Result<String, Box<dyn Error>>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("missing value for {flag}").into())
}

fn parse_ratio(value: &str, flag: &str) -> Result<f64, Box<dyn Error>> {
    let ratio: f64 = value.parse()?;
    if !(0.0..=1.0).contains(&ratio) {
        return Err(format!("{flag} must be between 0 and 1").into());
    }

    Ok(ratio)
}

fn parse_byte_limit(value: &str, flag: &str) -> Result<u64, Box<dyn Error>> {
    let trimmed = value.trim();
    let uppercase = trimmed.to_ascii_uppercase();
    let suffixes = [
        ("GB", 1_000_000_000_u64),
        ("MB", 1_000_000_u64),
        ("KB", 1_000_u64),
        ("B", 1_u64),
    ];

    for (suffix, multiplier) in suffixes {
        if uppercase.ends_with(suffix) {
            let number = trimmed[..trimmed.len() - suffix.len()].trim();
            let bytes: u64 = number.parse()?;
            return bytes
                .checked_mul(multiplier)
                .ok_or_else(|| format!("{flag} is too large").into());
        }
    }

    Ok(trimmed.parse()?)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, Box<dyn Error>> {
    if value == "now" {
        return Ok(Utc::now());
    }

    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn make_store_layer(
    number: usize,
    paths: &[String],
    conf: &Config,
    mtime: &str,
    layers_dir: &Path,
    blobs_dir: &Path,
) -> Result<Layer, Box<dyn Error>> {
    let store_prefix = format!("{}/", conf.store_dir);
    for path in paths {
        if !path.starts_with(&store_prefix) {
            return Err(format!(
                "oci-image-builder: store layer contains path outside {}: {}",
                conf.store_dir, path
            )
            .into());
        }
    }

    eprintln!("Creating layer {number} from paths: {}", paths.join(" "));

    let paths_file = layers_dir.join(format!("{number}.paths"));
    fs::write(&paths_file, paths.join("\n"))?;

    let layer_tmp = layers_dir.join(format!("{number}.layer.tar"));
    let HashedBytes { size, checksum } = write_tar_layer(&layer_tmp, &paths_file, conf, mtime)?;
    let tar_path = blobs_dir.join(&checksum);
    fs::rename(&layer_tmp, &tar_path)?;

    Ok(Layer {
        checksum,
        size,
        paths: paths.to_vec(),
        tar_path,
    })
}

fn make_customisation_layer(
    number: usize,
    customisation_layer: &str,
    blobs_dir: &Path,
) -> Result<Layer, Box<dyn Error>> {
    eprintln!("Creating layer {number} with customisation...");

    let customisation_layer = Path::new(customisation_layer);
    let checksum = fs::read_to_string(customisation_layer.join("checksum"))?
        .trim()
        .to_owned();
    if checksum.len() != 64 || !checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("oci-image-builder: invalid layer checksum: {checksum}").into());
    }

    let layer_path = customisation_layer.join("layer.tar");
    let size = fs::metadata(&layer_path)?.len();
    let tar_path = blobs_dir.join(&checksum);
    symlink(&layer_path, &tar_path)?;

    Ok(Layer {
        checksum,
        size,
        paths: vec![customisation_layer.display().to_string()],
        tar_path,
    })
}

fn analyze_layer_efficiency(layers: &[Layer]) -> Result<LayerEfficiency, Box<dyn Error>> {
    let mut entries = 0;
    let mut paths = BTreeMap::new();

    for (index, layer) in layers.iter().enumerate() {
        read_layer_entries(index + 1, &layer.tar_path, &mut entries, &mut paths)?;
    }

    let mut required_bytes = 0;
    let mut discovered_bytes = 0;
    let mut repeated_paths = 0;
    let mut inefficient_paths = Vec::new();

    for (path, stats) in &paths {
        required_bytes += stats.required_size;
        discovered_bytes += stats.cumulative_size;

        if stats.occurrences > 1 {
            repeated_paths += 1;
        }

        if stats.cumulative_size > stats.required_size {
            inefficient_paths.push(InefficientPath {
                path: path.clone(),
                occurrences: stats.occurrences,
                cumulative_size: stats.cumulative_size,
                required_size: stats.required_size,
            });
        }
    }

    inefficient_paths.sort_by(|left, right| {
        right
            .wasted_bytes()
            .cmp(&left.wasted_bytes())
            .then_with(|| left.path.cmp(&right.path))
    });

    let wasted_bytes = discovered_bytes.saturating_sub(required_bytes);
    let efficiency = ratio(required_bytes, discovered_bytes);
    let wasted_percent = ratio(wasted_bytes, discovered_bytes);

    Ok(LayerEfficiency {
        entries,
        paths: paths.len(),
        repeated_paths,
        discovered_bytes,
        required_bytes,
        wasted_bytes,
        efficiency,
        wasted_percent,
        inefficient_paths,
    })
}

fn read_layer_entries(
    layer_index: usize,
    tar_path: &Path,
    entries: &mut usize,
    paths: &mut BTreeMap<String, PathStats>,
) -> Result<(), Box<dyn Error>> {
    let file = File::open(tar_path)?;
    let mut archive = tar::Archive::new(file);

    for entry in archive.entries()? {
        let entry = entry?;
        *entries += 1;

        let path = String::from_utf8_lossy(entry.path_bytes().as_ref()).into_owned();
        if path.is_empty() {
            continue;
        }

        if let Some(whiteout) = whiteout_for(&path) {
            match whiteout {
                Whiteout::Remove(target) => remove_path(paths, &target, layer_index),
                Whiteout::Opaque(parent) => remove_children(paths, &parent, layer_index),
            }
            continue;
        }

        let entry_type = entry.header().entry_type();
        let size = if entry_type.is_file() {
            entry.header().size()?
        } else {
            0
        };
        record_path(paths, &path, size, layer_index);
    }

    Ok(())
}

fn whiteout_for(path: &str) -> Option<Whiteout> {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    if name == ".wh..wh..opq" {
        return Some(Whiteout::Opaque(parent.to_owned()));
    }

    let target_name = name.strip_prefix(".wh.")?;
    if parent.is_empty() {
        Some(Whiteout::Remove(target_name.to_owned()))
    } else {
        Some(Whiteout::Remove(format!("{parent}/{target_name}")))
    }
}

fn remove_path(paths: &mut BTreeMap<String, PathStats>, target: &str, layer_index: usize) {
    let prefix = format!("{target}/");
    for (path, stats) in paths {
        if (path == target || path.starts_with(&prefix)) && stats.last_layer < layer_index {
            stats.occurrences += 1;
            stats.required_size = 0;
            stats.last_layer = layer_index;
        }
    }
}

fn remove_children(paths: &mut BTreeMap<String, PathStats>, parent: &str, layer_index: usize) {
    let prefix = if parent.is_empty() {
        String::new()
    } else {
        format!("{parent}/")
    };

    for (path, stats) in paths {
        if path.starts_with(&prefix) && path != parent && stats.last_layer < layer_index {
            stats.occurrences += 1;
            stats.required_size = 0;
            stats.last_layer = layer_index;
        }
    }
}

fn record_path(paths: &mut BTreeMap<String, PathStats>, path: &str, size: u64, layer_index: usize) {
    match paths.entry(path.to_owned()) {
        Entry::Vacant(entry) => {
            entry.insert(PathStats {
                occurrences: 1,
                cumulative_size: size,
                required_size: size,
                last_layer: layer_index,
            });
        }
        Entry::Occupied(mut entry) => {
            let stats = entry.get_mut();
            stats.occurrences += 1;
            stats.cumulative_size += size;
            stats.required_size = size;
            stats.last_layer = layer_index;
        }
    }
}

fn report_layer_efficiency(
    efficiency: &LayerEfficiency,
    policy: EfficiencyPolicy,
) -> Result<(), Box<dyn Error>> {
    eprintln!(
        "OCI layer efficiency: score={} wasted={} ({}) required={} discovered={} entries={} paths={} repeated-paths={}",
        format_percent(efficiency.efficiency),
        format_bytes(efficiency.wasted_bytes),
        format_percent(efficiency.wasted_percent),
        format_bytes(efficiency.required_bytes),
        format_bytes(efficiency.discovered_bytes),
        efficiency.entries,
        efficiency.paths,
        efficiency.repeated_paths,
    );

    for path in efficiency.inefficient_paths.iter().take(policy.top_paths) {
        eprintln!(
            "  wasted={} count={} path={}",
            format_bytes(path.wasted_bytes()),
            path.occurrences,
            path.path,
        );
    }

    let mut failures = Vec::new();
    if efficiency.efficiency < policy.min_efficiency {
        failures.push(format!(
            "efficiency {} is below {}",
            format_percent(efficiency.efficiency),
            format_percent(policy.min_efficiency),
        ));
    }

    if efficiency.wasted_bytes > policy.max_wasted_bytes {
        failures.push(format!(
            "wasted bytes {} exceeds {}",
            format_bytes(efficiency.wasted_bytes),
            format_bytes(policy.max_wasted_bytes),
        ));
    }

    if efficiency.wasted_bytes > 0 && efficiency.wasted_percent >= policy.max_wasted_percent {
        failures.push(format!(
            "wasted percent {} meets or exceeds {}",
            format_percent(efficiency.wasted_percent),
            format_percent(policy.max_wasted_percent),
        ));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "oci-image-builder: OCI layer efficiency check failed: {}",
            failures.join("; ")
        )
        .into())
    }
}

#[allow(clippy::cast_precision_loss)]
fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        1.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn format_percent(value: f64) -> String {
    format!("{:.2}%", value * 100.0)
}

fn format_bytes(bytes: u64) -> String {
    let units = [
        ("GiB", 1024_u64.pow(3)),
        ("MiB", 1024_u64.pow(2)),
        ("KiB", 1024_u64),
    ];

    for (unit, divisor) in units {
        if bytes >= divisor {
            let whole = bytes / divisor;
            let decimal = bytes % divisor * 10 / divisor;
            return format!("{whole}.{decimal} {unit}");
        }
    }

    format!("{bytes} B")
}

fn write_metadata(
    conf: &Config,
    created: &str,
    layers: &[Layer],
    image_dir: &Path,
    mtime: &str,
    out_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let diff_ids: Vec<_> = layers
        .iter()
        .map(|layer| format!("sha256:{}", layer.checksum))
        .collect();
    let history: Vec<_> = layers
        .iter()
        .map(|layer| {
            serde_json::json!({
                "created": created,
                "comment": format!("store paths: {}", serde_json::to_string(&layer.paths).unwrap()),
            })
        })
        .collect();
    let image_config = serde_json::json!({
        "created": created,
        "architecture": conf.architecture,
        "os": "linux",
        "config": conf.settings,
        "rootfs": {
            "diff_ids": diff_ids,
            "type": "layers",
        },
        "history": history,
    });
    let image_config = serde_json::to_vec_pretty(&image_config)?;
    let config_checksum = sha256_bytes(&image_config);
    let config_size = image_config.len();
    fs::write(
        image_dir.join("blobs/sha256").join(&config_checksum),
        image_config,
    )?;

    let manifest_layers: Vec<_> = layers
        .iter()
        .map(|layer| {
            serde_json::json!({
                "mediaType": "application/vnd.oci.image.layer.v1.tar",
                "digest": format!("sha256:{}", layer.checksum),
                "size": layer.size,
            })
        })
        .collect();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": format!("sha256:{config_checksum}"),
            "size": config_size,
        },
        "layers": manifest_layers,
    });
    let manifest = serde_json::to_vec_pretty(&manifest)?;
    let manifest_checksum = sha256_bytes(&manifest);
    let manifest_size = manifest.len();
    fs::write(
        image_dir.join("blobs/sha256").join(&manifest_checksum),
        manifest,
    )?;

    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [
            {
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "digest": format!("sha256:{manifest_checksum}"),
                "size": manifest_size,
            }
        ],
    });
    fs::write(
        image_dir.join("index.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;

    let mtime_secs: u64 = mtime.parse()?;
    let mut entries: Vec<String> = vec!["oci-layout".to_owned(), "index.json".to_owned()];
    for entry in fs::read_dir(image_dir.join("blobs/sha256"))? {
        let entry = entry?;
        entries.push(format!(
            "blobs/sha256/{}",
            entry.file_name().to_string_lossy()
        ));
    }
    entries.sort();

    let out = File::create(out_path)?;
    let mut builder = tar::Builder::new(out);
    builder.follow_symlinks(false);
    for name in &entries {
        let source = image_dir.join(name);
        append_normalized_entry(&mut builder, &source, name, mtime_secs, 0, 0)?;
    }
    builder.finish()?;

    Ok(())
}

fn write_tar_layer(
    layer_path: &Path,
    paths_file: &Path,
    conf: &Config,
    mtime: &str,
) -> Result<HashedBytes, Box<dyn Error>> {
    let mtime_secs: u64 = mtime.parse()?;
    let uid: u64 = conf.uid.parse()?;
    let gid: u64 = conf.gid.parse()?;

    let mut paths: Vec<PathBuf> = vec![PathBuf::from("/nix"), PathBuf::from("/nix/store")];
    let paths_text = fs::read_to_string(paths_file)?;
    for line in paths_text.lines() {
        if line.is_empty() {
            continue;
        }
        collect_paths_recursive(Path::new(line), &mut paths)?;
    }
    // Matches GNU tar `--sort=name`: every entry, including the explicit
    // /nix and /nix/store roots, ends up in lexical order in the archive.
    paths.sort();
    paths.dedup();

    let layer = File::create(layer_path)?;
    let writer = HashingWriter::new(layer);
    let mut builder = tar::Builder::new(writer);
    builder.follow_symlinks(false);

    for path in &paths {
        let archive_name = path
            .strip_prefix("/")
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        append_normalized_entry(&mut builder, path, &archive_name, mtime_secs, uid, gid)?;
    }

    let writer = builder.into_inner()?;
    Ok(writer.finalize())
}

fn collect_paths_recursive(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    out.push(root.to_path_buf());
    let metadata = fs::symlink_metadata(root)?;
    let file_type = metadata.file_type();
    if !file_type.is_dir() || file_type.is_symlink() {
        return Ok(());
    }
    let mut children: Vec<PathBuf> = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();
    for child in children {
        collect_paths_recursive(&child, out)?;
    }
    Ok(())
}

fn append_normalized_entry<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &Path,
    archive_name: &str,
    mtime: u64,
    uid: u64,
    gid: u64,
) -> Result<(), Box<dyn Error>> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    let mode = metadata.permissions().mode() & 0o7777;

    let mut header = tar::Header::new_gnu();
    header.set_mtime(mtime);
    header.set_uid(uid);
    header.set_gid(gid);
    header.set_username("")?;
    header.set_groupname("")?;
    header.set_mode(mode);

    if file_type.is_symlink() {
        let link_target = fs::read_link(source)?;
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder.append_link(&mut header, archive_name, link_target)?;
    } else if file_type.is_dir() {
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        builder.append_data(&mut header, archive_name, io::empty())?;
    } else if file_type.is_file() {
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(metadata.len());
        // `--hard-dereference` is automatic here: every visit re-reads the
        // file content rather than emitting a hardlink reference, matching
        // the GNU tar flag the old shell-out used.
        let file = File::open(source)?;
        builder.append_data(&mut header, archive_name, file)?;
    } else {
        return Err(format!(
            "oci-image-builder: unsupported file type at {}",
            source.display()
        )
        .into());
    }

    Ok(())
}

struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    size: u64,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            size: 0,
        }
    }

    fn finalize(self) -> HashedBytes {
        let checksum = format!("{:x}", self.hasher.finalize());
        HashedBytes {
            size: self.size,
            checksum,
        }
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn sha256_bytes(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tar::{Builder, Header};

    #[test]
    fn layer_efficiency_counts_repeated_paths() -> Result<(), Box<dyn Error>> {
        let work = tempdir()?;
        let first = work.path().join("first.tar");
        let second = work.path().join("second.tar");
        write_test_tar(&first, &[("app/server.jar", b"first-version".as_slice())])?;
        write_test_tar(
            &second,
            &[
                ("app/server.jar", b"v2".as_slice()),
                ("app/other.txt", b"kept".as_slice()),
            ],
        )?;

        let efficiency =
            analyze_layer_efficiency(&[test_layer(first, 1)?, test_layer(second, 2)?])?;

        assert_eq!(efficiency.entries, 3);
        assert_eq!(efficiency.repeated_paths, 1);
        assert_eq!(efficiency.discovered_bytes, 19);
        assert_eq!(efficiency.required_bytes, 6);
        assert_eq!(efficiency.wasted_bytes, 13);
        assert_close(efficiency.efficiency, 6.0 / 19.0);
        assert_eq!(efficiency.inefficient_paths[0].path, "app/server.jar");
        assert_eq!(efficiency.inefficient_paths[0].wasted_bytes(), 13);

        Ok(())
    }

    #[test]
    fn layer_efficiency_counts_removed_paths() -> Result<(), Box<dyn Error>> {
        let work = tempdir()?;
        let first = work.path().join("first.tar");
        let second = work.path().join("second.tar");
        write_test_tar(&first, &[("srv/world.dat", b"world-state".as_slice())])?;
        write_test_tar(&second, &[("srv/.wh.world.dat", b"".as_slice())])?;

        let efficiency =
            analyze_layer_efficiency(&[test_layer(first, 1)?, test_layer(second, 2)?])?;

        assert_eq!(efficiency.discovered_bytes, 11);
        assert_eq!(efficiency.required_bytes, 0);
        assert_eq!(efficiency.wasted_bytes, 11);
        assert_eq!(efficiency.inefficient_paths[0].path, "srv/world.dat");

        Ok(())
    }

    #[test]
    fn efficiency_policy_rejects_excess_waste() {
        let efficiency = LayerEfficiency {
            entries: 2,
            paths: 1,
            repeated_paths: 1,
            discovered_bytes: 200,
            required_bytes: 100,
            wasted_bytes: 100,
            efficiency: 0.5,
            wasted_percent: 0.5,
            inefficient_paths: vec![InefficientPath {
                path: "/nix/store/example".to_owned(),
                occurrences: 2,
                cumulative_size: 200,
                required_size: 100,
            }],
        };

        let result = report_layer_efficiency(
            &efficiency,
            EfficiencyPolicy {
                min_efficiency: 0.95,
                max_wasted_bytes: 20,
                max_wasted_percent: 0.20,
                top_paths: 1,
            },
        );

        assert!(result.is_err());
    }

    fn test_layer(tar_path: PathBuf, number: usize) -> Result<Layer, Box<dyn Error>> {
        Ok(Layer {
            checksum: format!("{number:064x}"),
            size: fs::metadata(&tar_path)?.len(),
            paths: vec![tar_path.display().to_string()],
            tar_path,
        })
    }

    fn write_test_tar(path: &Path, files: &[(&str, &[u8])]) -> Result<(), Box<dyn Error>> {
        let file = File::create(path)?;
        let mut builder = Builder::new(file);

        for (name, contents) in files {
            let mut header = Header::new_gnu();
            header.set_path(name)?;
            header.set_size(contents.len().try_into()?);
            header.set_cksum();
            builder.append(&header, *contents)?;
        }

        builder.finish()?;
        Ok(())
    }

    fn assert_close(left: f64, right: f64) {
        assert!((left - right).abs() < f64::EPSILON);
    }
}
