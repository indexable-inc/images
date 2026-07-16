//! fio-based filesystem benchmark.
//!
//! Runs identical sequential/random fio workloads plus tiny-file
//! create/stat/delete phases against a target directory and reports
//! throughput, IOPS, p99 latency, and file rates. Run it twice (target vs a
//! disk-backed directory on the same machine) for a relative comparison.
//!
//! Rust port of the former `run.nu` (#3252). The output contract is
//! unchanged: `--json` prints one pretty-printed JSON document; the default
//! human report prints the same `Results` block.

use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};
use std::time::Instant;

use clap::Parser;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu, ensure};

/// Benchmark file-system behavior of a target directory with fio and
/// tiny-file metadata phases.
#[derive(Debug, Parser)]
#[command(name = "ix-bench-filesystem", version, about)]
struct Cli {
    /// Directory to benchmark (falls back to `$VCFS_BENCH_TARGET`).
    #[arg(long, value_name = "DIR")]
    target: Option<PathBuf>,
    /// Seconds per fio workload.
    #[arg(long, default_value_t = 8)]
    runtime: u32,
    /// fio ramp seconds excluded from stats.
    #[arg(long, default_value_t = 1)]
    ramp_time: u32,
    /// File size per fio workload.
    #[arg(long, default_value = "256m")]
    size: String,
    /// Tiny files for the create/stat/delete phases.
    #[arg(long, default_value_t = 5000)]
    files: u32,
    /// fio iodepth (sync ioengine).
    #[arg(long, default_value_t = 1)]
    iodepth: u32,
    /// Short sanity check: 2s runtime, 64m size, 1000 files.
    #[arg(long)]
    quick: bool,
    /// Machine-readable result on stdout.
    #[arg(long)]
    json: bool,
    /// Keep the scratch directory afterwards.
    #[arg(long)]
    keep: bool,
}

/// A failure raised while preparing or running the benchmark.
#[derive(Debug, Snafu)]
enum Error {
    /// Neither `--target` nor the environment variable named a directory.
    #[snafu(display("missing --target DIR or VCFS_BENCH_TARGET"))]
    MissingTarget,

    /// The target path does not exist.
    #[snafu(display("target does not exist: {}", path.display()))]
    TargetMissing {
        /// The path that was not found.
        path: PathBuf,
    },

    /// The target path exists but is not a directory.
    #[snafu(display("target is not a directory: {}", path.display()))]
    TargetNotDirectory {
        /// The non-directory path.
        path: PathBuf,
    },

    /// The target path could not be canonicalized.
    #[snafu(display("failed to resolve target {}", path.display()))]
    TargetResolve {
        /// The path that failed to resolve.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The scratch directory could not be created inside the target.
    #[snafu(display("failed to create scratch directory under {}", target.display()))]
    Scratch {
        /// The target the scratch directory was created under.
        target: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// fio could not be launched.
    #[snafu(display("failed to spawn fio"))]
    FioSpawn {
        /// Underlying error from `fork`/`exec`.
        source: std::io::Error,
    },

    /// A fio workload exited unsuccessfully.
    #[snafu(display("fio {name} exited unsuccessfully: {status}"))]
    FioFailed {
        /// The workload name.
        name: String,
        /// The fio exit status.
        status: ExitStatus,
    },

    /// A fio JSON output file could not be read.
    #[snafu(display("failed to read fio output at {}", path.display()))]
    FioRead {
        /// The unreadable output file.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A fio JSON output file did not parse.
    #[snafu(display("failed to parse fio output at {}", path.display()))]
    FioParse {
        /// The unparsable output file.
        path: PathBuf,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// A fio JSON output file carried no jobs.
    #[snafu(display("fio output at {} has no jobs", path.display()))]
    FioNoJobs {
        /// The jobless output file.
        path: PathBuf,
    },

    /// A metadata-phase filesystem operation failed.
    #[snafu(display("metadata {phase} failed at {}", path.display()))]
    Metadata {
        /// The phase that failed.
        phase: MetadataPhase,
        /// The path the operation touched.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// `/proc/self/mounts` (Linux) or `statfs` (Darwin) failed.
    #[snafu(display("failed to determine filesystem type of {}", path.display()))]
    FilesystemType {
        /// The probed path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// `uname` failed while stamping the report's `system` line.
    #[snafu(display("failed to read system identity"))]
    Uname {
        /// Underlying error from `nix::sys::utsname::uname`.
        source: nix::Error,
    },

    /// The result could not be serialized to JSON.
    #[snafu(display("failed to serialize result to JSON"))]
    Serialize {
        /// Underlying serde error.
        source: serde_json::Error,
    },
}

type Result<T, E = Error> = std::result::Result<T, E>;

/// Effective workload parameters after `--quick` is applied.
#[derive(Debug)]
struct Parameters {
    runtime: u32,
    ramp_time: u32,
    size: String,
    files: u32,
    iodepth: u32,
}

impl Parameters {
    fn from_cli(cli: &Cli) -> Self {
        if cli.quick {
            Self {
                runtime: 2,
                ramp_time: 0,
                size: "64m".to_owned(),
                files: 1000,
                iodepth: cli.iodepth,
            }
        } else {
            Self {
                runtime: cli.runtime,
                ramp_time: cli.ramp_time,
                size: cli.size.clone(),
                files: cli.files,
                iodepth: cli.iodepth,
            }
        }
    }
}

/// One fio workload: what varies between the five invocations.
#[derive(Debug)]
struct FioWorkload {
    name: &'static str,
    filename: &'static str,
    rw: &'static str,
    bs: &'static str,
    /// `--end_fsync=1`: include the final fsync in the measured window.
    end_fsync: bool,
    /// Timed measurement (`--time_based` + runtime/ramp) vs a plain
    /// write-the-whole-size pass (the read-source prefill).
    time_based: bool,
}

impl FioWorkload {
    /// A workload measured over a timed window (`--time_based` plus the
    /// runtime/ramp parameters).
    const fn timed(
        name: &'static str,
        filename: &'static str,
        rw: &'static str,
        bs: &'static str,
        end_fsync: bool,
    ) -> Self {
        Self {
            name,
            filename,
            rw,
            bs,
            end_fsync,
            time_based: true,
        }
    }

    /// The untimed pass that writes `read-source.dat` to its full size so
    /// the read workloads consume real data.
    const fn prefill() -> Self {
        Self {
            name: "prefill-read-source",
            filename: "read-source.dat",
            rw: "write",
            bs: "1m",
            end_fsync: true,
            time_based: false,
        }
    }
}

/// The one renderer for fio's `--key=value` argv format.
fn fio_args(scratch: &Path, parameters: &Parameters, workload: &FioWorkload) -> Vec<String> {
    let output = scratch.join(format!("{}.json", workload.name));
    let mut args = vec![
        format!("--name={}", workload.name),
        format!("--directory={}", scratch.display()),
        format!("--filename={}", workload.filename),
        format!("--output={}", output.display()),
        "--output-format=json".to_owned(),
        "--ioengine=sync".to_owned(),
        format!("--iodepth={}", parameters.iodepth),
        "--numjobs=1".to_owned(),
        "--thread=1".to_owned(),
        "--group_reporting=1".to_owned(),
    ];
    if workload.time_based {
        args.push("--time_based=1".to_owned());
        args.push(format!("--runtime={}", parameters.runtime));
        args.push(format!("--ramp_time={}", parameters.ramp_time));
    }
    args.push(format!("--size={}", parameters.size));
    args.push(format!("--rw={}", workload.rw));
    args.push(format!("--bs={}", workload.bs));
    if workload.end_fsync {
        args.push("--end_fsync=1".to_owned());
    }
    args
}

fn run_fio(scratch: &Path, parameters: &Parameters, workload: &FioWorkload) -> Result<()> {
    let status = Command::new("fio")
        .args(fio_args(scratch, parameters, workload))
        .status()
        .context(FioSpawnSnafu)?;
    ensure!(
        status.success(),
        FioFailedSnafu {
            name: workload.name,
            status,
        }
    );
    Ok(())
}

/// The slice of fio's JSON output this benchmark reads. Every field defaults
/// to zero when absent (a read-only job reports no `write` side and vice
/// versa), matching the previous script's `| default 0` chain.
#[derive(Debug, Default, Deserialize)]
struct FioOutput {
    #[serde(default)]
    jobs: Vec<FioJob>,
}

#[derive(Debug, Default, Deserialize)]
struct FioJob {
    #[serde(default)]
    read: FioDirection,
    #[serde(default)]
    write: FioDirection,
}

#[derive(Debug, Default, Deserialize)]
struct FioDirection {
    #[serde(default)]
    iops: f64,
    #[serde(default)]
    bw_bytes: u64,
    #[serde(default)]
    clat_ns: FioLatency,
}

#[derive(Debug, Default, Deserialize)]
struct FioLatency {
    #[serde(default)]
    mean: f64,
    #[serde(default)]
    percentile: std::collections::BTreeMap<String, u64>,
}

/// fio's key for the 99th percentile in `clat_ns.percentile`.
const P99_KEY: &str = "99.000000";

/// One direction (read or write) of a fio workload summary, in the report's
/// stable field names.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FioSide {
    iops: f64,
    bandwidth_bytes_per_second: u64,
    mean_latency_ns: f64,
    p99_latency_ns: u64,
}

impl FioSide {
    fn from_direction(direction: &FioDirection) -> Self {
        Self {
            iops: direction.iops,
            bandwidth_bytes_per_second: direction.bw_bytes,
            mean_latency_ns: direction.clat_ns.mean,
            p99_latency_ns: direction
                .clat_ns
                .percentile
                .get(P99_KEY)
                .copied()
                .unwrap_or(0),
        }
    }
}

/// Summary of one fio workload, read back from its JSON output file.
#[derive(Debug, Serialize)]
struct FioSummary {
    name: &'static str,
    read: FioSide,
    write: FioSide,
}

fn fio_summary(scratch: &Path, name: &'static str) -> Result<FioSummary> {
    let path = scratch.join(format!("{name}.json"));
    let raw = fs::read_to_string(&path).context(FioReadSnafu { path: &path })?;
    let output: FioOutput = serde_json::from_str(&raw).context(FioParseSnafu { path: &path })?;
    let job = output
        .jobs
        .first()
        .context(FioNoJobsSnafu { path: &path })?;
    Ok(FioSummary {
        name,
        read: FioSide::from_direction(&job.read),
        write: FioSide::from_direction(&job.write),
    })
}

/// A metadata phase over many tiny files.
#[derive(Debug, Clone, Copy)]
enum MetadataPhase {
    Create,
    Stat,
    Delete,
}

impl fmt::Display for MetadataPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Create => "create",
            Self::Stat => "stat",
            Self::Delete => "delete",
        })
    }
}

/// Summary of one metadata phase.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetadataSummary {
    name: String,
    files: u32,
    seconds: f64,
    files_per_second: f64,
}

/// coreutils `sync PATH` semantics: fsync an fd opened on the path, falling
/// back to a whole-system `sync(2)` when that fails (the previous script's
/// `try { sync $path } catch { sync }`), e.g. on filesystems that reject
/// directory fsync.
fn sync_path(path: &Path) {
    if File::open(path).and_then(|file| file.sync_all()).is_err() {
        unsafe { libc::sync() };
    }
}

fn metadata_bench(scratch: &Path, phase: MetadataPhase, files: u32) -> Result<MetadataSummary> {
    let dir = scratch.join("metadata");
    let context = |path: &Path| MetadataSnafu {
        phase,
        path: path.to_path_buf(),
    };
    fs::create_dir_all(&dir).context(context(&dir))?;
    let start = Instant::now();
    match phase {
        MetadataPhase::Create => {
            for i in 1..=files {
                let path = dir.join(format!("file-{i}"));
                fs::write(&path, format!("ix-bench-{i}\n")).context(context(&path))?;
            }
            sync_path(&dir);
        }
        MetadataPhase::Stat => {
            for entry in fs::read_dir(&dir).context(context(&dir))? {
                let entry = entry.context(context(&dir))?;
                // One stat per regular file, like `find -type f -exec stat`.
                let metadata = entry.metadata().context(context(&entry.path()))?;
                debug_assert!(metadata.is_file());
            }
        }
        MetadataPhase::Delete => {
            for entry in fs::read_dir(&dir).context(context(&dir))? {
                let entry = entry.context(context(&dir))?;
                fs::remove_file(entry.path()).context(context(&entry.path()))?;
            }
            fs::remove_dir(&dir).context(context(&dir))?;
            sync_path(scratch);
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(MetadataSummary {
        name: format!("metadata-{phase}"),
        files,
        seconds,
        files_per_second: if seconds == 0.0 {
            0.0
        } else {
            f64::from(files) / seconds
        },
    })
}

/// The filesystem type of the mount containing `target`.
///
/// Linux: the fstype the kernel reports in `/proc/self/mounts` for the
/// longest mount-point prefix of the (already canonical) target. The previous
/// script shelled to GNU `stat -f -c %T`, which maps statfs magic numbers to
/// coarser names (ext4 reports as `ext2/ext3`); the mount table names the
/// filesystem directly (`ext4`, `virtiofs`, ...).
#[cfg(target_os = "linux")]
fn filesystem_type(target: &Path) -> Result<String> {
    let path = Path::new("/proc/self/mounts");
    let mounts = fs::read_to_string(path).context(FilesystemTypeSnafu { path })?;
    let mut best: Option<(usize, &str)> = None;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_device), Some(mount_point), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // Mount points with whitespace are octal-escaped in the mount table;
        // benchmarking inside one is out of scope, so no unescaping here.
        if target.starts_with(mount_point)
            && best.is_none_or(|(depth, _)| mount_point.len() > depth)
        {
            best = Some((mount_point.len(), fstype));
        }
    }
    Ok(best
        .map(|(_, fstype)| fstype.to_owned())
        .unwrap_or_default())
}

/// The filesystem type of the mount containing `target`, from statfs's
/// `f_fstypename` (`apfs`, `nfs`, ...).
#[cfg(not(target_os = "linux"))]
fn filesystem_type(target: &Path) -> Result<String> {
    let statfs = nix::sys::statfs::statfs(target)
        .map_err(std::io::Error::from)
        .context(FilesystemTypeSnafu { path: target })?;
    Ok(statfs.filesystem_type_name().to_owned())
}

/// A `uname -a`-style identity line: sysname, nodename, release, version,
/// machine.
fn system_identity() -> Result<String> {
    let info = nix::sys::utsname::uname().context(UnameSnafu)?;
    Ok([
        info.sysname(),
        info.nodename(),
        info.release(),
        info.version(),
        info.machine(),
    ]
    .map(|part| part.to_string_lossy())
    .join(" "))
}

/// Workload parameters echoed into the report.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportParameters {
    size: String,
    runtime_seconds: u32,
    ramp_time_seconds: u32,
    iodepth: u32,
    metadata_files: u32,
}

/// The full benchmark result: the `--json` document.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    generated_at: String,
    target: String,
    scratch: String,
    filesystem: String,
    system: String,
    parameters: ReportParameters,
    fio: Vec<FioSummary>,
    metadata: Vec<MetadataSummary>,
}

/// Nushell `math floor` semantics for the human report's display values.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "display floor of small non-negative rates"
)]
const fn floor(value: f64) -> u64 {
    value.max(0.0).floor() as u64
}

const BYTES_PER_MIB: u64 = 1024 * 1024;
const NS_PER_MS: u64 = 1_000_000;

fn print_human(report: &Report) {
    let fio = |name: &str| -> &FioSummary {
        report
            .fio
            .iter()
            .find(|summary| summary.name == name)
            .expect("every fio workload is recorded before printing")
    };
    let metadata = |name: &str| -> &MetadataSummary {
        report
            .metadata
            .iter()
            .find(|summary| summary.name == name)
            .expect("every metadata phase is recorded before printing")
    };
    let throughput = |side: &FioSide| -> String {
        format!(
            "{} MiB/s, p99 {} ms",
            side.bandwidth_bytes_per_second / BYTES_PER_MIB,
            side.p99_latency_ns / NS_PER_MS
        )
    };
    let iops = |side: &FioSide| -> String {
        format!(
            "{} IOPS, p99 {} ms",
            floor(side.iops),
            side.p99_latency_ns / NS_PER_MS
        )
    };

    println!();
    println!("Results");
    println!("  seq-write:  {}", throughput(&fio("seq-write").write));
    println!("  seq-read:   {}", throughput(&fio("seq-read").read));
    println!("  rand-write: {}", iops(&fio("rand-write").write));
    println!("  rand-read:  {}", iops(&fio("rand-read").read));
    println!(
        "  create:     {} files/s",
        floor(metadata("metadata-create").files_per_second)
    );
    println!(
        "  stat:       {} files/s",
        floor(metadata("metadata-stat").files_per_second)
    );
    println!(
        "  delete:     {} files/s",
        floor(metadata("metadata-delete").files_per_second)
    );
}

/// Run the five fio invocations in order, announcing each on stdout unless
/// `--json` silences the human narration.
fn run_workloads(scratch: &Path, parameters: &Parameters, json: bool) -> Result<()> {
    let workloads = [
        (
            "running seq-write...",
            FioWorkload::timed("seq-write", "seq-write.dat", "write", "1m", true),
        ),
        (
            "running rand-write...",
            FioWorkload::timed("rand-write", "rand-write.dat", "randwrite", "4k", true),
        ),
        ("prefilling read source...", FioWorkload::prefill()),
        (
            "running seq-read...",
            FioWorkload::timed("seq-read", "read-source.dat", "read", "1m", false),
        ),
        (
            "running rand-read...",
            FioWorkload::timed("rand-read", "read-source.dat", "randread", "4k", false),
        ),
    ];
    for (message, workload) in workloads {
        if !json {
            println!("{message}");
        }
        run_fio(scratch, parameters, &workload)?;
    }
    Ok(())
}

fn resolve_target(flag: Option<&Path>) -> Result<PathBuf> {
    let raw = match flag {
        Some(path) if !path.as_os_str().is_empty() => path.to_owned(),
        _ => match std::env::var_os("VCFS_BENCH_TARGET") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => return MissingTargetSnafu.fail(),
        },
    };
    // The previous script's `path expand`: absolute with symlinks resolved.
    let target = match fs::canonicalize(&raw) {
        Ok(target) => target,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return TargetMissingSnafu { path: raw }.fail();
        }
        Err(source) => return Err(source).context(TargetResolveSnafu { path: raw }),
    };
    ensure!(target.is_dir(), TargetNotDirectorySnafu { path: target });
    Ok(target)
}

fn run(cli: &Cli) -> Result<()> {
    let target = resolve_target(cli.target.as_deref())?;
    let parameters = Parameters::from_cli(cli);

    // Dropped on error or when `--keep` is absent, removing the fio data
    // files and JSON outputs; `--keep` leaks it deliberately below.
    let scratch_dir = tempfile::Builder::new()
        .prefix(".ix-fs-bench.")
        .tempdir_in(&target)
        .context(ScratchSnafu { target: &target })?;
    let scratch = scratch_dir.path().to_path_buf();

    if !cli.json {
        println!("ix filesystem benchmark");
        println!("target: {}", target.display());
        println!("scratch: {}", scratch.display());
        println!("runtime: {}s per fio workload", parameters.runtime);
        println!("size: {} per fio workload", parameters.size);
        println!("metadata files: {}", parameters.files);
        println!();
    }

    run_workloads(&scratch, &parameters, cli.json)?;

    let mut metadata = Vec::with_capacity(3);
    for phase in [
        MetadataPhase::Create,
        MetadataPhase::Stat,
        MetadataPhase::Delete,
    ] {
        if !cli.json {
            println!("running metadata-{phase}...");
        }
        metadata.push(metadata_bench(&scratch, phase, parameters.files)?);
    }

    let report = Report {
        // The previous script stamped local time with a hardcoded `Z`; this
        // keeps the format but makes the value genuinely UTC.
        generated_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        target: target.display().to_string(),
        scratch: scratch.display().to_string(),
        filesystem: filesystem_type(&target)?,
        system: system_identity()?,
        parameters: ReportParameters {
            size: parameters.size.clone(),
            runtime_seconds: parameters.runtime,
            ramp_time_seconds: parameters.ramp_time,
            iodepth: parameters.iodepth,
            metadata_files: parameters.files,
        },
        fio: vec![
            fio_summary(&scratch, "seq-write")?,
            fio_summary(&scratch, "rand-write")?,
            fio_summary(&scratch, "seq-read")?,
            fio_summary(&scratch, "rand-read")?,
        ],
        metadata,
    };

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context(SerializeSnafu)?
        );
    } else {
        print_human(&report);
    }

    if cli.keep {
        let kept = scratch_dir.keep();
        if !cli.json {
            println!();
            println!("scratch kept at: {}", kept.display());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", snafu::Report::from_error(error));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert `value` exposes every key in `keys`.
    fn assert_keys(value: &serde_json::Value, keys: &[&str]) {
        for key in keys {
            assert!(value.get(key).is_some(), "missing key {key}");
        }
    }

    fn parameters() -> Parameters {
        Parameters {
            runtime: 8,
            ramp_time: 1,
            size: "256m".to_owned(),
            files: 5000,
            iodepth: 1,
        }
    }

    #[test]
    fn fio_args_render_timed_fsync_workload() {
        let args = fio_args(
            Path::new("/scratch"),
            &parameters(),
            &FioWorkload::timed("seq-write", "seq-write.dat", "write", "1m", true),
        );
        assert_eq!(
            args,
            [
                "--name=seq-write",
                "--directory=/scratch",
                "--filename=seq-write.dat",
                "--output=/scratch/seq-write.json",
                "--output-format=json",
                "--ioengine=sync",
                "--iodepth=1",
                "--numjobs=1",
                "--thread=1",
                "--group_reporting=1",
                "--time_based=1",
                "--runtime=8",
                "--ramp_time=1",
                "--size=256m",
                "--rw=write",
                "--bs=1m",
                "--end_fsync=1",
            ]
        );
    }

    #[test]
    fn fio_args_render_untimed_prefill() {
        let args = fio_args(
            Path::new("/scratch"),
            &parameters(),
            &FioWorkload::prefill(),
        );
        assert!(!args.iter().any(|arg| arg.starts_with("--time_based")));
        assert!(!args.iter().any(|arg| arg.starts_with("--runtime")));
        assert!(!args.iter().any(|arg| arg.starts_with("--ramp_time")));
        assert!(args.contains(&"--end_fsync=1".to_owned()));
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "exact fixture values round-trip through serde"
    )]
    fn fio_summary_reads_both_directions_and_defaults_missing() {
        let scratch = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            scratch.path().join("seq-write.json"),
            r#"{
              "jobs": [{
                "write": {
                  "iops": 123.5,
                  "bw_bytes": 1048576,
                  "clat_ns": {"mean": 250.5, "percentile": {"99.000000": 4000}}
                }
              }]
            }"#,
        )
        .expect("write fixture");

        let summary = fio_summary(scratch.path(), "seq-write").expect("summary");
        assert_eq!(summary.write.iops, 123.5);
        assert_eq!(summary.write.bandwidth_bytes_per_second, 1_048_576);
        assert_eq!(summary.write.mean_latency_ns, 250.5);
        assert_eq!(summary.write.p99_latency_ns, 4000);
        // The read side is absent from the fixture: every field defaults to 0.
        assert_eq!(summary.read.iops, 0.0);
        assert_eq!(summary.read.bandwidth_bytes_per_second, 0);
        assert_eq!(summary.read.p99_latency_ns, 0);
    }

    #[test]
    fn metadata_bench_round_trips_create_stat_delete() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let files = 10;

        let create = metadata_bench(scratch.path(), MetadataPhase::Create, files).expect("create");
        assert_eq!(create.name, "metadata-create");
        assert_eq!(create.files, files);
        assert!(scratch.path().join("metadata").join("file-10").is_file());

        let stat = metadata_bench(scratch.path(), MetadataPhase::Stat, files).expect("stat");
        assert_eq!(stat.name, "metadata-stat");

        let delete = metadata_bench(scratch.path(), MetadataPhase::Delete, files).expect("delete");
        assert_eq!(delete.name, "metadata-delete");
        assert!(!scratch.path().join("metadata").exists());
        assert!(delete.files_per_second > 0.0);
    }

    #[test]
    fn report_serializes_with_stable_field_names() {
        let report = Report {
            generated_at: "2026-01-01T00:00:00Z".to_owned(),
            target: "/t".to_owned(),
            scratch: "/t/.ix-fs-bench.abc".to_owned(),
            filesystem: "ext4".to_owned(),
            system: "Linux host 6.6 v x86_64".to_owned(),
            parameters: ReportParameters {
                size: "256m".to_owned(),
                runtime_seconds: 8,
                ramp_time_seconds: 1,
                iodepth: 1,
                metadata_files: 5000,
            },
            fio: vec![FioSummary {
                name: "seq-write",
                read: FioSide::from_direction(&FioDirection::default()),
                write: FioSide::from_direction(&FioDirection::default()),
            }],
            metadata: vec![MetadataSummary {
                name: "metadata-create".to_owned(),
                files: 5000,
                seconds: 1.5,
                files_per_second: 3333.3,
            }],
        };
        let value = serde_json::to_value(&report).expect("serialize");
        assert_keys(
            &value,
            &[
                "generatedAt",
                "target",
                "scratch",
                "filesystem",
                "system",
                "parameters",
                "fio",
                "metadata",
            ],
        );
        assert_keys(
            &value["parameters"],
            &[
                "size",
                "runtimeSeconds",
                "rampTimeSeconds",
                "iodepth",
                "metadataFiles",
            ],
        );
        assert_keys(
            &value["fio"][0]["write"],
            &[
                "iops",
                "bandwidthBytesPerSecond",
                "meanLatencyNs",
                "p99LatencyNs",
            ],
        );
        assert_keys(
            &value["metadata"][0],
            &["name", "files", "seconds", "filesPerSecond"],
        );
    }

    #[test]
    fn resolve_target_rejects_a_file() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let file = scratch.path().join("plain-file");
        std::fs::write(&file, "x").expect("write");
        let error = resolve_target(Some(&file)).expect_err("file target must be rejected");
        assert!(matches!(error, Error::TargetNotDirectory { .. }));
    }

    #[test]
    fn resolve_target_rejects_a_missing_path() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let missing = scratch.path().join("nope");
        let error = resolve_target(Some(&missing)).expect_err("missing target must be rejected");
        assert!(matches!(error, Error::TargetMissing { .. }));
    }

    #[test]
    fn floor_matches_nushell_math_floor() {
        assert_eq!(floor(0.0), 0);
        assert_eq!(floor(1234.9), 1234);
        assert_eq!(floor(-1.0), 0);
    }
}
