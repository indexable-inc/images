use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet, btree_map::Entry};
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use tempfile::tempdir;

const DEFAULT_MIN_EFFICIENCY: f64 = 0.95;
const DEFAULT_MAX_WASTED_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_MAX_WASTED_PERCENT: f64 = 0.20;
const DEFAULT_EFFICIENCY_TOP_PATHS: usize = 10;

struct Args {
    mode: Mode,
    input: PathBuf,
    output: PathBuf,
    efficiency_policy: Option<EfficiencyPolicy>,
}

/// What the builder does with its input.
///
/// - `Build`: a layer plan (`conf.json`) to a finished OCI tar. The legacy
///   one-shot, kept so the NixOS image path is unchanged.
/// - `Describe`: a layer plan to an `image.json` description, writing no layer
///   blobs. The durable, content-addressed artifact.
/// - `Materialize`: an `image.json` back to a finished OCI tar, regenerating
///   each layer's bytes deterministically and verifying them against the
///   description's digests.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Build,
    Describe,
    Materialize,
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

#[derive(Clone)]
struct Layer {
    checksum: String,
    size: u64,
    paths: Vec<String>,
    tar_path: PathBuf,
}

/// The content-addressed description of an image: everything needed to
/// regenerate the exact OCI archive without storing any layer bytes. This is
/// the artifact `describe` emits and `materialize` consumes. Layer bytes are
/// reproduced on demand from `source`; only digests and sizes are recorded.
#[derive(Serialize, Deserialize)]
struct Description {
    schema_version: u32,
    architecture: String,
    created: String,
    mtime: String,
    uid: String,
    gid: String,
    store_dir: String,
    config: Value,
    layers: Vec<LayerDesc>,
}

/// One layer in a `Description`: its identity (`digest`/`diff_id`/`size`) plus
/// the `source` that regenerates its bytes byte-for-byte. For uncompressed tar
/// layers the blob digest equals the diff id.
#[derive(Serialize, Deserialize)]
struct LayerDesc {
    digest: String,
    diff_id: String,
    size: u64,
    #[serde(flatten)]
    source: LayerSource,
}

/// How a layer's bytes are regenerated at materialize time.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LayerSource {
    /// Re-tar a set of Nix store paths (deterministic from the paths).
    Store { paths: Vec<String> },
    /// Copy a layer member out of a base docker-archive.
    Base { archive: PathBuf, member: String },
    /// Copy the prebuilt customisation layer tar from its derivation output.
    Customisation { dir: PathBuf },
}

/// The cached description of a pulled base image: its layer descriptions plus
/// the base container config to overlay under the final image config. Produced
/// by `base-desc` from the immutable, digest-pinned base archive, so it is built
/// once and reused across every image and rebuild that shares that base.
#[derive(Serialize, Deserialize)]
struct BaseDesc {
    layers: Vec<LayerDesc>,
    config: Value,
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
    let cli: Vec<String> = env::args().collect();

    // Per-layer sharding lives outside the `Mode` enum: `layer-desc` describes a
    // single store layer (the unit the Nix build puts in its own derivation) and
    // `assemble-desc` stitches the precomputed store-layer descriptions into a
    // full `image.json` without re-tarring them. Both take their own argument
    // shape, so they are dispatched before the legacy positional parser.
    match cli.get(1).map(String::as_str) {
        Some("layer-desc") => return run_layer_desc(&cli[2..]),
        Some("base-desc") => return run_base_desc(&cli[2..]),
        Some("assemble-desc") => return run_assemble_desc(&cli[2..]),
        _ => {}
    }

    let parsed = parse_args(cli)?;
    match parsed.mode {
        Mode::Build => run_build(&parsed.input, &parsed.output, parsed.efficiency_policy),
        Mode::Describe => run_describe(&parsed.input, &parsed.output, parsed.efficiency_policy),
        Mode::Materialize => {
            run_materialize(&parsed.input, &parsed.output, parsed.efficiency_policy)
        }
    }
}

/// Normalize an mtime that may arrive as RFC3339 (the `conf.json` shape) or as
/// bare unix seconds, to the seconds string `write_tar_layer` expects.
fn mtime_seconds(value: &str) -> String {
    parse_time(value).map_or_else(|_| value.to_owned(), |time| time.timestamp().to_string())
}

/// Describe one store layer: tar the given store paths, hash the bytes, and
/// write a single-layer JSON recording `{digest, diff_id, size}` plus the paths
/// that regenerate it. The Nix build runs one of these per layer, so editing one
/// store path re-tars only its layer and the rest are derivation cache hits. The
/// digest matches what a one-shot `describe` records for the same inputs, which
/// is what lets `assemble-desc` splice these in without re-tarring.
fn run_layer_desc(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut uid: Option<String> = None;
    let mut gid: Option<String> = None;
    let mut mtime: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--uid" => uid = Some(iter.next().ok_or("missing value for --uid")?.clone()),
            "--gid" => gid = Some(iter.next().ok_or("missing value for --gid")?.clone()),
            "--mtime" => mtime = Some(iter.next().ok_or("missing value for --mtime")?.clone()),
            _ if arg.starts_with('-') => return Err(format!("unknown argument: {arg}").into()),
            _ => positional.push(arg.clone()),
        }
    }

    let uid: u64 = uid.ok_or("layer-desc: missing --uid")?.parse()?;
    let gid: u64 = gid.ok_or("layer-desc: missing --gid")?.parse()?;
    let mtime = mtime_seconds(&mtime.ok_or("layer-desc: missing --mtime")?);

    if positional.is_empty() {
        return Err("usage: oci-image-builder layer-desc --uid <n> --gid <n> --mtime <secs> <out.json> <store-path>...".into());
    }
    let out_path = PathBuf::from(positional.remove(0));
    let paths = positional;

    let work = tempdir()?;
    let paths_file = work.path().join("paths");
    fs::write(&paths_file, paths.join("\n"))?;
    let tar_tmp = work.path().join("layer.tar");
    let HashedBytes { size, checksum } = write_tar_layer(&tar_tmp, &paths_file, uid, gid, &mtime)?;

    let desc = LayerDesc {
        digest: format!("sha256:{checksum}"),
        diff_id: format!("sha256:{checksum}"),
        size,
        source: LayerSource::Store { paths },
    };
    fs::write(out_path, serde_json::to_vec_pretty(&desc)?)?;
    Ok(())
}

/// Assemble a full `image.json` from a layer plan plus the precomputed
/// store-layer descriptions, without re-tarring the store layers. Base layers
/// (pulled, immutable) and the customisation layer are hashed here because they
/// are cheap and not the churn; the store layers carry the closure and arrive
/// straight from their per-layer derivation outputs, in plan order. The output
/// is byte-identical to a one-shot `describe`.
fn run_base_desc(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.len() != 2 {
        return Err("usage: oci-image-builder base-desc <base-archive.tar> <out base.json>".into());
    }
    let archive = PathBuf::from(&args[0]);
    let out_path = PathBuf::from(&args[1]);

    // Hash the base layers in a scratch dir that is dropped on return; only the
    // digests and the base config are kept. The base archive is pinned by digest
    // and immutable, so this derivation never reruns on a closure change.
    let work = image_work()?;
    let base = load_base_image(&archive, &work.layers_dir, &work.blobs_dir)?;
    let layers = base
        .layers
        .iter()
        .map(|layer| LayerDesc {
            digest: format!("sha256:{}", layer.checksum),
            diff_id: format!("sha256:{}", layer.checksum),
            size: layer.size,
            source: LayerSource::Base {
                archive: archive.clone(),
                member: layer.paths.first().cloned().unwrap_or_default(),
            },
        })
        .collect();

    let base_desc = BaseDesc {
        layers,
        config: base.config,
    };
    eprintln!("Describing {} base layer(s)...", base_desc.layers.len());
    fs::write(out_path, serde_json::to_vec_pretty(&base_desc)?)?;
    Ok(())
}

/// Stitch a full `image.json` from precomputed parts: the cached base
/// description, one description per store layer, and the customisation layer.
/// Nothing is re-tarred here. The store layers carry the closure and the churn,
/// and arrive from their per-layer derivation outputs; the base is read from its
/// own cached derivation. With every input cached this is pure JSON assembly, so
/// a one-layer change costs one layer re-tar plus this near-instant stitch.
fn run_assemble_desc(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut base_file: Option<PathBuf> = None;
    let mut positional: Vec<PathBuf> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--base" => {
                base_file = Some(PathBuf::from(
                    iter.next().ok_or("missing value for --base")?,
                ));
            }
            _ if arg.starts_with('-') => return Err(format!("unknown argument: {arg}").into()),
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    if positional.len() < 2 {
        return Err("usage: oci-image-builder assemble-desc --base <base.json> <conf.json> <out image.json> <store-layer.json>...".into());
    }
    let conf_path = &positional[0];
    let out_path = &positional[1];
    let store_layer_files = &positional[2..];

    let conf: Config = serde_json::from_reader(File::open(conf_path)?)?;
    let created = parse_time(&conf.created)?.to_rfc3339_opts(SecondsFormat::Secs, false);
    let mtime = parse_time(&conf.mtime)?.timestamp().to_string();

    if store_layer_files.len() != conf.store_layers.len() {
        return Err(format!(
            "assemble-desc: got {} store-layer descriptions but the plan has {} store layers",
            store_layer_files.len(),
            conf.store_layers.len()
        )
        .into());
    }

    // Base layers (bottom of the stack) from the cached base description, or the
    // empty set for a pure Nix closure with no `fromImage`.
    let base: BaseDesc = match base_file {
        Some(path) => serde_json::from_reader(File::open(path)?)?,
        None => BaseDesc {
            layers: Vec::new(),
            config: Value::Null,
        },
    };
    let mut layers = base.layers;

    // Store layers in plan order. Verify each description really describes the
    // planned paths rather than trusting the argument order.
    for (file, plan_paths) in store_layer_files.iter().zip(&conf.store_layers) {
        let desc: LayerDesc = serde_json::from_reader(File::open(file)?)?;
        match &desc.source {
            LayerSource::Store { paths } if paths == plan_paths => {}
            LayerSource::Store { .. } => {
                return Err(format!(
                    "assemble-desc: store-layer {} paths do not match the plan",
                    file.display()
                )
                .into());
            }
            _ => {
                return Err(
                    format!("assemble-desc: {} is not a store layer", file.display()).into(),
                );
            }
        }
        layers.push(desc);
    }

    // Customisation layer on top. It is a prebuilt derivation output carrying its
    // own checksum, so describing it reads a file rather than tarring anything.
    let cust = make_customisation_layer(0, &conf.customisation_layer, &image_work()?.blobs_dir)?;
    layers.push(LayerDesc {
        digest: format!("sha256:{}", cust.checksum),
        diff_id: format!("sha256:{}", cust.checksum),
        size: cust.size,
        source: LayerSource::Customisation {
            dir: PathBuf::from(&conf.customisation_layer),
        },
    });

    let description = Description {
        schema_version: 1,
        architecture: conf.architecture,
        created,
        mtime,
        uid: conf.uid,
        gid: conf.gid,
        store_dir: conf.store_dir,
        config: merge_config(&conf.settings, &base.config),
        layers,
    };

    eprintln!(
        "Assembling image description from {} store layers...",
        store_layer_files.len()
    );
    fs::write(out_path, serde_json::to_vec_pretty(&description)?)?;
    eprintln!("Done.");
    Ok(())
}

/// Layers and config assembled from a layer plan, plus the count of base layers
/// at the bottom (excluded from efficiency analysis) and the `source` for each
/// layer so the same set can be serialized into a `Description`.
struct Assembled {
    layers: Vec<Layer>,
    sources: Vec<LayerSource>,
    settings: Value,
    base_layer_count: usize,
}

/// Create the OCI image scaffold (`oci-layout`, `blobs/sha256`, a scratch layer
/// dir) under a fresh temp tree, returning it so the caller can populate it.
struct ImageWork {
    _root: tempfile::TempDir,
    image_dir: PathBuf,
    blobs_dir: PathBuf,
    layers_dir: PathBuf,
}

fn image_work() -> Result<ImageWork, Box<dyn Error>> {
    let root = tempdir()?;
    let image_dir = root.path().join("image");
    let blobs_dir = image_dir.join("blobs/sha256");
    let layers_dir = root.path().join("layers");
    fs::create_dir_all(&blobs_dir)?;
    fs::create_dir_all(&layers_dir)?;
    fs::write(
        image_dir.join("oci-layout"),
        r#"{"imageLayoutVersion":"1.0.0"}"#,
    )?;
    Ok(ImageWork {
        _root: root,
        image_dir,
        blobs_dir,
        layers_dir,
    })
}

/// Build every layer from a layer plan into content-addressed blobs, recording
/// how each layer regenerates. A non-Nix base (`fromImage`) contributes its
/// layers at the bottom of the stack and its environment to the final config;
/// with no base the image is a pure Nix closure.
fn assemble(
    conf: &Config,
    mtime: &str,
    layers_dir: &Path,
    blobs_dir: &Path,
) -> Result<Assembled, Box<dyn Error>> {
    let base = load_base(conf, layers_dir, blobs_dir)?;
    let base_layer_count = base.layers.len();
    let from_image = conf.from_image.as_str().map(PathBuf::from);

    let mut sources: Vec<LayerSource> = Vec::new();
    for layer in &base.layers {
        let archive = from_image
            .clone()
            .ok_or("oci-image-builder: base layer present without a from_image path")?;
        sources.push(LayerSource::Base {
            archive,
            member: layer.paths.first().cloned().unwrap_or_default(),
        });
    }

    let mut layers = base.layers;
    for (index, paths) in conf.store_layers.iter().enumerate() {
        layers.push(make_store_layer(
            base_layer_count + index + 1,
            paths,
            conf,
            mtime,
            layers_dir,
            blobs_dir,
        )?);
        sources.push(LayerSource::Store {
            paths: paths.clone(),
        });
    }

    layers.push(make_customisation_layer(
        base_layer_count + conf.store_layers.len() + 1,
        &conf.customisation_layer,
        blobs_dir,
    )?);
    sources.push(LayerSource::Customisation {
        dir: PathBuf::from(&conf.customisation_layer),
    });

    Ok(Assembled {
        layers,
        sources,
        settings: merge_config(&conf.settings, &base.config),
        base_layer_count,
    })
}

/// Only police the layers we generate. Base layers are pulled and immutable, so
/// their internal duplication is not ours to fix and would otherwise fail every
/// image built on a fat base.
fn check_efficiency(
    assembled: &Assembled,
    policy: Option<EfficiencyPolicy>,
) -> Result<(), Box<dyn Error>> {
    if let Some(policy) = policy {
        let efficiency = analyze_layer_efficiency(&assembled.layers[assembled.base_layer_count..])?;
        report_layer_efficiency(&efficiency, policy)?;
    }
    Ok(())
}

/// Legacy one-shot: layer plan straight to a finished OCI tar.
fn run_build(
    conf_path: &Path,
    out_path: &Path,
    efficiency_policy: Option<EfficiencyPolicy>,
) -> Result<(), Box<dyn Error>> {
    let conf: Config = serde_json::from_reader(File::open(conf_path)?)?;
    let created = parse_time(&conf.created)?.to_rfc3339_opts(SecondsFormat::Secs, false);
    let mtime = parse_time(&conf.mtime)?.timestamp().to_string();

    let work = image_work()?;
    let assembled = assemble(&conf, &mtime, &work.layers_dir, &work.blobs_dir)?;
    check_efficiency(&assembled, efficiency_policy)?;

    eprintln!("Adding manifests...");
    write_metadata(
        &conf.architecture,
        &assembled.settings,
        &created,
        &assembled.layers,
        &work.image_dir,
        &mtime,
        out_path,
    )?;
    eprintln!("Done.");
    Ok(())
}

/// Emit a content-addressed `image.json` and discard the layer bytes. The
/// blobs are hashed (and the efficiency policy enforced) in a scratch dir that
/// is dropped on return, so nothing multi-gigabyte lands in the store.
fn run_describe(
    conf_path: &Path,
    out_path: &Path,
    efficiency_policy: Option<EfficiencyPolicy>,
) -> Result<(), Box<dyn Error>> {
    let conf: Config = serde_json::from_reader(File::open(conf_path)?)?;
    let created = parse_time(&conf.created)?.to_rfc3339_opts(SecondsFormat::Secs, false);
    let mtime = parse_time(&conf.mtime)?.timestamp().to_string();

    let work = image_work()?;
    let assembled = assemble(&conf, &mtime, &work.layers_dir, &work.blobs_dir)?;
    check_efficiency(&assembled, efficiency_policy)?;

    let layers = assembled
        .layers
        .iter()
        .zip(assembled.sources)
        .map(|(layer, source)| LayerDesc {
            digest: format!("sha256:{}", layer.checksum),
            diff_id: format!("sha256:{}", layer.checksum),
            size: layer.size,
            source,
        })
        .collect();
    let description = Description {
        schema_version: 1,
        architecture: conf.architecture,
        created,
        mtime,
        uid: conf.uid,
        gid: conf.gid,
        store_dir: conf.store_dir,
        config: assembled.settings,
        layers,
    };

    eprintln!("Writing image description...");
    fs::write(out_path, serde_json::to_vec_pretty(&description)?)?;
    eprintln!("Done.");
    Ok(())
}

/// Regenerate the OCI tar from an `image.json`. Each layer's bytes are
/// reproduced from its `source` and verified against the recorded digest, so a
/// description that no longer reproduces its bytes fails the build instead of
/// shipping a wrong image.
fn run_materialize(
    json_path: &Path,
    out_path: &Path,
    efficiency_policy: Option<EfficiencyPolicy>,
) -> Result<(), Box<dyn Error>> {
    let description: Description = serde_json::from_reader(File::open(json_path)?)?;
    let uid: u64 = description.uid.parse()?;
    let gid: u64 = description.gid.parse()?;

    let work = image_work()?;
    let mut layers = Vec::with_capacity(description.layers.len());
    for (index, desc) in description.layers.iter().enumerate() {
        let expected = desc.digest.strip_prefix("sha256:").unwrap_or(&desc.digest);
        eprintln!("Materializing layer {} ({expected})", index + 1);
        let layer = regenerate_layer(
            desc,
            expected,
            &description.mtime,
            uid,
            gid,
            &work.layers_dir,
            &work.blobs_dir,
        )?;
        layers.push(layer);
    }

    // The describe path shards layers across derivations and cannot run the
    // cross-layer efficiency analysis, so enforce the policy here, where the
    // regenerated bytes already exist at no extra tar cost. Police every layer
    // we generate (store layers plus the customisation layer), matching the
    // legacy one-shot gate; only the pulled, immutable base layers are excluded,
    // since their internal duplication is not ours to fix.
    if let Some(policy) = efficiency_policy {
        let generated_layers: Vec<Layer> = layers
            .iter()
            .zip(&description.layers)
            .filter(|(_, desc)| !matches!(desc.source, LayerSource::Base { .. }))
            .map(|(layer, _)| layer.clone())
            .collect();
        let efficiency = analyze_layer_efficiency(&generated_layers)?;
        report_layer_efficiency(&efficiency, policy)?;
    }

    eprintln!("Adding manifests...");
    write_metadata(
        &description.architecture,
        &description.config,
        &description.created,
        &layers,
        &work.image_dir,
        &description.mtime,
        out_path,
    )?;
    eprintln!("Done.");
    Ok(())
}

/// Reproduce one layer's bytes from its `source`, write the blob, and check the
/// resulting digest matches the description.
fn regenerate_layer(
    desc: &LayerDesc,
    expected: &str,
    mtime: &str,
    uid: u64,
    gid: u64,
    layers_dir: &Path,
    blobs_dir: &Path,
) -> Result<Layer, Box<dyn Error>> {
    let layer = match &desc.source {
        LayerSource::Store { paths } => {
            let paths_file = layers_dir.join(format!("{expected}.paths"));
            fs::write(&paths_file, paths.join("\n"))?;
            let tmp = layers_dir.join(format!("{expected}.tar"));
            let HashedBytes { size, checksum } =
                write_tar_layer(&tmp, &paths_file, uid, gid, mtime)?;
            let tar_path = blobs_dir.join(&checksum);
            fs::rename(&tmp, &tar_path)?;
            Layer {
                checksum,
                size,
                paths: paths.clone(),
                tar_path,
            }
        }
        LayerSource::Base { archive, member } => {
            regenerate_base_member(archive, member, layers_dir, blobs_dir)?
        }
        LayerSource::Customisation { dir } => {
            make_customisation_layer(0, &dir.to_string_lossy(), blobs_dir)?
        }
    };

    if layer.checksum != expected {
        return Err(format!(
            "oci-image-builder: materialized layer digest mismatch: \
             description {expected}, regenerated {}",
            layer.checksum
        )
        .into());
    }
    Ok(layer)
}

/// Copy a single layer member out of a base docker-archive into a blob.
fn regenerate_base_member(
    archive: &Path,
    member: &str,
    layers_dir: &Path,
    blobs_dir: &Path,
) -> Result<Layer, Box<dyn Error>> {
    for entry in tar::Archive::new(File::open(archive)?).entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        if name != member {
            continue;
        }
        let tmp = layers_dir.join(format!("base-{}", sha256_bytes(member.as_bytes())));
        let mut writer = HashingWriter::new(File::create(&tmp)?);
        io::copy(&mut entry, &mut writer)?;
        let HashedBytes { size, checksum } = writer.finalize();
        let tar_path = blobs_dir.join(&checksum);
        fs::rename(&tmp, &tar_path)?;
        return Ok(Layer {
            checksum,
            size,
            paths: vec![member.to_owned()],
            tar_path,
        });
    }
    Err(format!(
        "oci-image-builder: base member {member} not found in {}",
        archive.display()
    )
    .into())
}

/// A non-Nix base image resolved into content-addressed layers plus the base
/// container config to overlay under the final image config.
#[derive(Default)]
struct BaseImage {
    layers: Vec<Layer>,
    config: Value,
}

/// The parts of a base docker-archive needed to assemble its layers: the layer
/// member names in stack order, their declared `diff_ids`, and the base
/// container config (`Entrypoint`, `Cmd`, `Env`, `WorkingDir`, `User`, ...).
struct BaseManifest {
    layer_names: Vec<String>,
    diff_ids: Vec<String>,
    config: Value,
}

/// Resolve the `fromImage` field: a string path to a base docker-archive, or
/// null for a pure Nix closure. Any other shape is a typed error rather than a
/// silent "no base" fallback.
fn load_base(
    conf: &Config,
    layers_dir: &Path,
    blobs_dir: &Path,
) -> Result<BaseImage, Box<dyn Error>> {
    if let Some(path) = conf.from_image.as_str() {
        return load_base_image(Path::new(path), layers_dir, blobs_dir);
    }
    if !conf.from_image.is_null() {
        return Err("oci-image-builder: from_image must be a string path or null".into());
    }
    eprintln!("No 'fromImage' provided");
    Ok(BaseImage::default())
}

/// Load a non-Nix base image (a docker-archive tarball, as produced by
/// `dockerTools.pullImage`) into content-addressed blobs.
///
/// Each layer's bytes are verified against the `diff_ids` the base config
/// declares; a mismatch fails the build rather than shipping a base whose
/// content does not match its digest. skopeo writes docker-archive layers
/// uncompressed, so the blob digest equals the `diff_id`. A gzipped base layer
/// would fail this check, which is the correct signal to add compression
/// handling rather than silently emit a wrong image.
fn load_base_image(
    from_image: &Path,
    layers_dir: &Path,
    blobs_dir: &Path,
) -> Result<BaseImage, Box<dyn Error>> {
    let manifest = read_base_manifest(from_image)?;
    let mut extracted =
        extract_base_layers(from_image, &manifest.layer_names, layers_dir, blobs_dir)?;

    // Assemble in manifest order, verifying each layer against its diff_id.
    let mut layers = Vec::with_capacity(manifest.layer_names.len());
    for (index, layer_name) in manifest.layer_names.iter().enumerate() {
        let layer = extracted.remove(layer_name).ok_or_else(|| {
            format!("oci-image-builder: base layer {layer_name} not found in archive")
        })?;
        if let Some(expected) = manifest.diff_ids.get(index) {
            let expected = expected.strip_prefix("sha256:").unwrap_or(expected);
            if expected != layer.checksum {
                return Err(format!(
                    "oci-image-builder: base layer {layer_name} digest mismatch: \
                     manifest diff_id {expected}, computed {}",
                    layer.checksum
                )
                .into());
            }
        }
        eprintln!("Adding base layer {} from {layer_name}", index + 1);
        layers.push(layer);
    }

    Ok(BaseImage {
        layers,
        config: manifest.config,
    })
}

/// Read the base docker-archive's `manifest.json` and the config JSON it points
/// at. Both are small text members, and the config name is only known after the
/// manifest is parsed, so buffer every JSON member in one pass.
fn read_base_manifest(from_image: &Path) -> Result<BaseManifest, Box<dyn Error>> {
    let mut json_members: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for entry in tar::Archive::new(File::open(from_image)?).entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        let is_json = Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
        if name == "manifest.json" || is_json {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            json_members.insert(name, buf);
        }
    }

    let manifest_bytes = json_members
        .get("manifest.json")
        .ok_or("oci-image-builder: base image is missing manifest.json")?;
    let manifest: Value = serde_json::from_slice(manifest_bytes)?;
    let entry = manifest
        .get(0)
        .ok_or("oci-image-builder: base image manifest.json is empty")?;

    let layer_names = entry
        .get("Layers")
        .and_then(Value::as_array)
        .ok_or("oci-image-builder: base image manifest has no Layers")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "oci-image-builder: base image Layers entry is not a string".into())
        })
        .collect::<Result<Vec<String>, Box<dyn Error>>>()?;

    let config_name = entry
        .get("Config")
        .and_then(Value::as_str)
        .ok_or("oci-image-builder: base image manifest has no Config")?;
    let config_bytes = json_members
        .get(config_name)
        .ok_or_else(|| format!("oci-image-builder: base image config {config_name} not found"))?;
    let config: Value = serde_json::from_slice(config_bytes)?;

    Ok(BaseManifest {
        layer_names,
        diff_ids: string_array(config.pointer("/rootfs/diff_ids")),
        config: config.pointer("/config").cloned().unwrap_or(Value::Null),
    })
}

/// Extract each referenced base layer into a blob named by its content digest,
/// returning a map from the archive member name to its resolved layer.
fn extract_base_layers(
    from_image: &Path,
    layer_names: &[String],
    layers_dir: &Path,
    blobs_dir: &Path,
) -> Result<BTreeMap<String, Layer>, Box<dyn Error>> {
    let want: HashSet<&str> = layer_names.iter().map(String::as_str).collect();
    let mut extracted: BTreeMap<String, Layer> = BTreeMap::new();
    for (index, entry) in tar::Archive::new(File::open(from_image)?)
        .entries()?
        .enumerate()
    {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        if !want.contains(name.as_str()) {
            continue;
        }
        let tmp = layers_dir.join(format!("base-{index}.tar"));
        let mut writer = HashingWriter::new(File::create(&tmp)?);
        io::copy(&mut entry, &mut writer)?;
        let HashedBytes { size, checksum } = writer.finalize();
        let tar_path = blobs_dir.join(&checksum);
        fs::rename(&tmp, &tar_path)?;
        extracted.insert(
            name.clone(),
            Layer {
                checksum,
                size,
                paths: vec![name],
                tar_path,
            },
        );
    }
    Ok(extracted)
}

/// Collect a JSON array of strings into a `Vec<String>`, dropping non-string
/// entries. Missing or non-array values yield an empty vector.
fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Overlay the base image config under the final config: every key the base
/// sets (`Entrypoint`, `Cmd`, `WorkingDir`, `User`, `ExposedPorts`, ...) is kept
/// unless the final config overrides it, so a base that carries an entrypoint or
/// working directory does not silently lose it. `Env` is concat-merged (base
/// first, the final image winning per variable, first-seen order preserved)
/// rather than replaced, mirroring nixpkgs' `stream_layered_image.py`.
fn merge_config(settings: &Value, base_config: &Value) -> Value {
    let Some(base) = base_config.as_object() else {
        return settings.clone();
    };

    let mut merged = base.clone();
    if let Some(object) = settings.as_object() {
        for (key, value) in object {
            merged.insert(key.clone(), value.clone());
        }
    }

    let env = merge_env_lists(
        &string_array(base_config.pointer("/Env")),
        &string_array(settings.pointer("/Env")),
    );
    if env.is_empty() {
        merged.remove("Env");
    } else {
        merged.insert("Env".to_owned(), Value::Array(env));
    }

    Value::Object(merged)
}

/// Concat-merge two `Env` lists: base entries first, the final list winning on
/// key collision, each variable keeping the position of its first appearance.
fn merge_env_lists(base_env: &[String], final_env: &[String]) -> Vec<Value> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, String> = HashMap::new();
    for entry in base_env.iter().chain(final_env.iter()) {
        let key = entry.split_once('=').map_or(entry.as_str(), |(key, _)| key);
        if !latest.contains_key(key) {
            order.push(key.to_owned());
        }
        latest.insert(key.to_owned(), entry.clone());
    }
    order
        .into_iter()
        .map(|key| Value::String(latest.remove(&key).unwrap_or_default()))
        .collect()
}

fn parse_args<I>(args: I) -> Result<Args, Box<dyn Error>>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let program = args
        .first()
        .map_or_else(|| "oci-image-builder".to_owned(), String::to_owned);

    // An optional leading subcommand selects the mode; without one the tool runs
    // the legacy plan-to-tar build so the NixOS image path is unchanged.
    let (mode, skip) = match args.get(1).map(String::as_str) {
        Some("describe") => (Mode::Describe, 2),
        Some("materialize") => (Mode::Materialize, 2),
        _ => (Mode::Build, 1),
    };

    let mut iter = args.into_iter().skip(skip);
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
            "usage: {program} [describe|materialize] [--skip-efficiency-check] [--min-efficiency <ratio>] [--max-wasted-bytes <bytes>] [--max-wasted-percent <ratio>] [--efficiency-top-paths <count>] <input> <output>"
        )
        .into());
    }

    Ok(Args {
        mode,
        input: positional.remove(0),
        output: positional.remove(0),
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
    let uid: u64 = conf.uid.parse()?;
    let gid: u64 = conf.gid.parse()?;
    let HashedBytes { size, checksum } = write_tar_layer(&layer_tmp, &paths_file, uid, gid, mtime)?;
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
    architecture: &str,
    settings: &Value,
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
        "architecture": architecture,
        "os": "linux",
        "config": settings,
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
    uid: u64,
    gid: u64,
    mtime: &str,
) -> Result<HashedBytes, Box<dyn Error>> {
    let mtime_secs: u64 = mtime.parse()?;

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
        let checksum = hex::encode(self.hasher.finalize());
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
    hex::encode(Sha256::digest(data))
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

    /// Build an uncompressed layer tar in memory, the same shape `pullImage`
    /// stores inside a docker-archive.
    fn layer_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        for (name, contents) in files {
            let mut header = Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(contents.len() as u64);
            header.set_cksum();
            builder.append(&header, *contents).unwrap();
        }
        builder.into_inner().unwrap()
    }

    /// Write a docker-archive: a flat tar of named members (manifest.json, the
    /// config JSON, and the layer tars).
    fn write_docker_archive(
        path: &Path,
        members: &[(&str, Vec<u8>)],
    ) -> Result<(), Box<dyn Error>> {
        let mut builder = Builder::new(File::create(path)?);
        for (name, contents) in members {
            let mut header = Header::new_gnu();
            header.set_path(name)?;
            header.set_size(contents.len() as u64);
            header.set_cksum();
            builder.append(&header, contents.as_slice())?;
        }
        builder.finish()?;
        Ok(())
    }

    #[test]
    fn load_base_image_extracts_layers_in_order() -> Result<(), Box<dyn Error>> {
        let work = tempdir()?;
        let layer_a = layer_tar(&[("usr/bin/a", b"aaa")]);
        let layer_b = layer_tar(&[("usr/bin/b", b"bbbb")]);
        let sha_a = sha256_bytes(&layer_a);
        let sha_b = sha256_bytes(&layer_b);

        let config = serde_json::json!({
            "architecture": "amd64",
            "os": "linux",
            "rootfs": {
                "type": "layers",
                "diff_ids": [format!("sha256:{sha_a}"), format!("sha256:{sha_b}")],
            },
            "config": { "Env": ["PATH=/usr/bin", "FOO=base"] },
        });
        let config_bytes = serde_json::to_vec(&config)?;
        let config_name = format!("{}.json", sha256_bytes(&config_bytes));
        let manifest = serde_json::json!([{
            "Config": config_name,
            "RepoTags": ["base:probe"],
            "Layers": ["a.tar", "b.tar"],
        }]);
        let archive = work.path().join("base.tar");
        write_docker_archive(
            &archive,
            &[
                ("manifest.json", serde_json::to_vec(&manifest)?),
                (config_name.as_str(), config_bytes),
                ("a.tar", layer_a),
                ("b.tar", layer_b),
            ],
        )?;
        let layers_dir = work.path().join("layers");
        let blobs_dir = work.path().join("blobs");
        fs::create_dir_all(&layers_dir)?;
        fs::create_dir_all(&blobs_dir)?;

        let base = load_base_image(&archive, &layers_dir, &blobs_dir)?;

        assert_eq!(base.layers.len(), 2);
        assert_eq!(base.layers[0].checksum, sha_a);
        assert_eq!(base.layers[1].checksum, sha_b);
        assert!(blobs_dir.join(&sha_a).exists());
        assert!(blobs_dir.join(&sha_b).exists());
        let env: Vec<&str> = base.config["Env"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(env, ["PATH=/usr/bin", "FOO=base"]);
        Ok(())
    }

    #[test]
    fn load_base_image_rejects_digest_mismatch() -> Result<(), Box<dyn Error>> {
        let work = tempdir()?;
        let layer_a = layer_tar(&[("usr/bin/a", b"aaa")]);
        // diff_id claims a digest the layer bytes do not hash to.
        let config = serde_json::json!({
            "rootfs": { "type": "layers", "diff_ids": ["sha256:deadbeef"] },
            "config": {},
        });
        let config_bytes = serde_json::to_vec(&config)?;
        let config_name = format!("{}.json", sha256_bytes(&config_bytes));
        let manifest = serde_json::json!([{
            "Config": config_name,
            "Layers": ["a.tar"],
        }]);
        let archive = work.path().join("base.tar");
        write_docker_archive(
            &archive,
            &[
                ("manifest.json", serde_json::to_vec(&manifest)?),
                (config_name.as_str(), config_bytes),
                ("a.tar", layer_a),
            ],
        )?;
        let layers_dir = work.path().join("layers");
        let blobs_dir = work.path().join("blobs");
        fs::create_dir_all(&layers_dir)?;
        fs::create_dir_all(&blobs_dir)?;

        let result = load_base_image(&archive, &layers_dir, &blobs_dir);

        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("digest mismatch"))
        );
        Ok(())
    }

    #[test]
    fn merge_config_overlays_base_under_final() {
        let settings = serde_json::json!({
            "Entrypoint": ["/init"],
            "Env": ["FOO=final", "BAR=final"],
        });
        let base = serde_json::json!({
            "Entrypoint": ["/base-entry"],
            "Cmd": ["serve"],
            "WorkingDir": "/srv",
            "Env": ["PATH=/usr/bin", "FOO=base"],
        });

        let merged = merge_config(&settings, &base);

        let env: Vec<&str> = merged["Env"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        // base order first (PATH, FOO), then final-only (BAR); FOO wins from final.
        assert_eq!(env, ["PATH=/usr/bin", "FOO=final", "BAR=final"]);
        // The final config overrides Entrypoint but inherits Cmd and WorkingDir
        // from the base instead of dropping them.
        assert_eq!(merged["Entrypoint"][0], "/init");
        assert_eq!(merged["Cmd"][0], "serve");
        assert_eq!(merged["WorkingDir"], "/srv");
    }

    #[test]
    fn merge_config_without_base_is_identity() {
        let settings = serde_json::json!({ "Env": ["A=1"] });
        assert_eq!(merge_config(&settings, &Value::Null), settings);
    }

    /// Read a member out of a tar archive by exact name.
    fn read_member(tar_path: &Path, name: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        for entry in tar::Archive::new(File::open(tar_path)?).entries()? {
            let mut entry = entry?;
            if entry.path()?.to_string_lossy() == name {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                return Ok(buf);
            }
        }
        Err(format!("member {name} not found in {}", tar_path.display()).into())
    }

    /// The ordered layer digests from an OCI archive's manifest.
    fn oci_layer_digests(tar_path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
        let index: Value = serde_json::from_slice(&read_member(tar_path, "index.json")?)?;
        let manifest_digest = index["manifests"][0]["digest"].as_str().unwrap();
        let blob = format!(
            "blobs/sha256/{}",
            manifest_digest.strip_prefix("sha256:").unwrap()
        );
        let manifest: Value = serde_json::from_slice(&read_member(tar_path, &blob)?)?;
        Ok(manifest["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| layer["digest"].as_str().unwrap().to_owned())
            .collect())
    }

    fn make_customisation_dir(dir: &Path) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(dir)?;
        let tar = layer_tar(&[("app/run", b"hi")]);
        fs::write(dir.join("layer.tar"), &tar)?;
        fs::write(dir.join("checksum"), sha256_bytes(&tar))?;
        Ok(())
    }

    fn single_layer_base(path: &Path) -> Result<(), Box<dyn Error>> {
        let layer = layer_tar(&[("usr/bin/a", b"aaa")]);
        let sha = sha256_bytes(&layer);
        let config = serde_json::json!({
            "rootfs": { "type": "layers", "diff_ids": [format!("sha256:{sha}")] },
            "config": {},
        });
        let config_bytes = serde_json::to_vec(&config)?;
        let config_name = format!("{}.json", sha256_bytes(&config_bytes));
        let manifest = serde_json::json!([{ "Config": config_name, "Layers": ["a.tar"] }]);
        write_docker_archive(
            path,
            &[
                ("manifest.json", serde_json::to_vec(&manifest)?),
                (config_name.as_str(), config_bytes),
                ("a.tar", layer),
            ],
        )
    }

    /// The core invariant: `describe` then `materialize` reproduces exactly the
    /// layers a one-shot `build` produces, across all three source kinds (a
    /// pulled base layer, a Nix store layer, the customisation layer).
    #[test]
    fn describe_then_materialize_matches_build() -> Result<(), Box<dyn Error>> {
        let work = tempdir()?;

        let store_dir = work.path().join("store");
        let pkg = store_dir.join("pkg");
        fs::create_dir_all(&pkg)?;
        fs::write(pkg.join("file"), b"hello")?;

        let cust = work.path().join("cust");
        make_customisation_dir(&cust)?;

        let base = work.path().join("base.tar");
        single_layer_base(&base)?;

        let plan = serde_json::json!({
            "architecture": "amd64",
            "config": { "Cmd": ["/bin/sh"] },
            "from_image": base.to_string_lossy(),
            "store_layers": [[pkg.to_string_lossy()]],
            "customisation_layer": cust.to_string_lossy(),
            "created": "1970-01-01T00:00:01Z",
            "mtime": "1970-01-01T00:00:01Z",
            "uid": "0",
            "gid": "0",
            "store_dir": store_dir.to_string_lossy(),
        });
        let conf = work.path().join("conf.json");
        fs::write(&conf, serde_json::to_vec(&plan)?)?;

        let built = work.path().join("build.tar");
        run_build(&conf, &built, None)?;

        let image_json = work.path().join("image.json");
        run_describe(&conf, &image_json, None)?;
        let materialized = work.path().join("materialized.tar");
        run_materialize(&image_json, &materialized, None)?;

        let build_digests = oci_layer_digests(&built)?;
        let materialize_digests = oci_layer_digests(&materialized)?;
        assert_eq!(build_digests.len(), 3, "base + store + customisation");
        assert_eq!(build_digests, materialize_digests);

        let description: Description = serde_json::from_slice(&fs::read(&image_json)?)?;
        let described: Vec<String> = description
            .layers
            .iter()
            .map(|layer| layer.digest.clone())
            .collect();
        assert_eq!(described, build_digests);
        Ok(())
    }

    /// Sharding describe across `layer-desc` (one per store layer) and stitching
    /// with `assemble-desc` must produce the exact same `image.json` bytes as a
    /// one-shot `describe`. That byte-identity is what lets the Nix build cache
    /// each layer in its own derivation without changing the materialized image.
    #[test]
    fn assemble_desc_matches_describe() -> Result<(), Box<dyn Error>> {
        let work = tempdir()?;

        let store_dir = work.path().join("store");
        let pkg_a = store_dir.join("pkg-a");
        let pkg_b = store_dir.join("pkg-b");
        fs::create_dir_all(&pkg_a)?;
        fs::create_dir_all(&pkg_b)?;
        fs::write(pkg_a.join("file"), b"hello")?;
        fs::write(pkg_b.join("file"), b"world")?;

        let cust = work.path().join("cust");
        make_customisation_dir(&cust)?;
        let base = work.path().join("base.tar");
        single_layer_base(&base)?;

        let plan = serde_json::json!({
            "architecture": "amd64",
            "config": { "Cmd": ["/bin/sh"] },
            "from_image": base.to_string_lossy(),
            "store_layers": [[pkg_a.to_string_lossy()], [pkg_b.to_string_lossy()]],
            "customisation_layer": cust.to_string_lossy(),
            "created": "1970-01-01T00:00:01Z",
            "mtime": "1970-01-01T00:00:01Z",
            "uid": "0",
            "gid": "0",
            "store_dir": store_dir.to_string_lossy(),
        });
        let conf = work.path().join("conf.json");
        fs::write(&conf, serde_json::to_vec(&plan)?)?;

        // One-shot describe, the baseline.
        let one_shot = work.path().join("one-shot.json");
        run_describe(&conf, &one_shot, None)?;

        // Per-layer: one `layer-desc` per store layer, then `assemble-desc`.
        let layer_a = work.path().join("layer-a.json");
        let layer_b = work.path().join("layer-b.json");
        run_layer_desc(&[
            "--uid".into(),
            "0".into(),
            "--gid".into(),
            "0".into(),
            "--mtime".into(),
            "1970-01-01T00:00:01Z".into(),
            layer_a.to_string_lossy().into_owned(),
            pkg_a.to_string_lossy().into_owned(),
        ])?;
        run_layer_desc(&[
            "--uid".into(),
            "0".into(),
            "--gid".into(),
            "0".into(),
            "--mtime".into(),
            "1970-01-01T00:00:01Z".into(),
            layer_b.to_string_lossy().into_owned(),
            pkg_b.to_string_lossy().into_owned(),
        ])?;

        // Cache the base description (built once from the immutable base archive).
        let base_desc = work.path().join("base.json");
        run_base_desc(&[
            base.to_string_lossy().into_owned(),
            base_desc.to_string_lossy().into_owned(),
        ])?;

        let sharded = work.path().join("sharded.json");
        run_assemble_desc(&[
            "--base".into(),
            base_desc.to_string_lossy().into_owned(),
            conf.to_string_lossy().into_owned(),
            sharded.to_string_lossy().into_owned(),
            layer_a.to_string_lossy().into_owned(),
            layer_b.to_string_lossy().into_owned(),
        ])?;

        assert_eq!(
            fs::read(&one_shot)?,
            fs::read(&sharded)?,
            "sharded image.json must be byte-identical to one-shot describe"
        );

        // And the sharded description still materializes to the same image.
        let mat_one = work.path().join("one.tar");
        let mat_sharded = work.path().join("sharded.tar");
        run_materialize(&one_shot, &mat_one, None)?;
        run_materialize(&sharded, &mat_sharded, None)?;
        assert_eq!(
            oci_layer_digests(&mat_one)?,
            oci_layer_digests(&mat_sharded)?
        );
        Ok(())
    }

    #[test]
    fn materialize_rejects_tampered_digest() -> Result<(), Box<dyn Error>> {
        let work = tempdir()?;
        let cust = work.path().join("cust");
        make_customisation_dir(&cust)?;

        let description = Description {
            schema_version: 1,
            architecture: "amd64".to_owned(),
            created: "1970-01-01T00:00:01Z".to_owned(),
            mtime: "1".to_owned(),
            uid: "0".to_owned(),
            gid: "0".to_owned(),
            store_dir: "/nix/store".to_owned(),
            config: serde_json::json!({}),
            layers: vec![LayerDesc {
                digest: "sha256:deadbeef".to_owned(),
                diff_id: "sha256:deadbeef".to_owned(),
                size: 0,
                source: LayerSource::Customisation { dir: cust },
            }],
        };
        let json = work.path().join("image.json");
        fs::write(&json, serde_json::to_vec(&description)?)?;

        let result = run_materialize(&json, &work.path().join("out.tar"), None);

        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("digest mismatch"))
        );
        Ok(())
    }
}
