//! Pipeline execution: spawn the stages, tee every stream through a bounded
//! [`Capture`], preserve streaming and backpressure.
//!
//! Every inter-stage edge is a copy thread reading the upstream stage's
//! stdout pipe and writing both the capture and the downstream stage's
//! stdin. When the downstream half closes (EPIPE), the thread stops reading
//! so the upstream process takes SIGPIPE exactly as it would under bash
//! (`yes | head` must terminate).

use std::fs::{File, OpenOptions};
use std::io::{PipeReader, PipeWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::Instant;

use snafu::ResultExt;

use crate::error::{Error, IoSnafu, RedirectSnafu};
use crate::report::{Capture, PipelineRun, Stage};

/// Where the first stage's stdin comes from.
pub enum StdinSpec {
    /// `/dev/null`: commands must not wait on a terminal that isn't there.
    Null,
    /// `< file`
    File(PathBuf),
}

/// A `>`/`>>`-style file sink.
pub struct FileSink {
    pub path: PathBuf,
    pub append: bool,
}

/// One stage, resolved and expanded, ready to run.
pub enum StageSpec {
    /// An external process.
    External(Box<ExternalSpec>),
    /// Builtin `echo`: pure output, safe anywhere in a pipeline.
    Echo {
        /// Bytes to emit (trailing newline already applied unless `-n`).
        text: Vec<u8>,
        /// The argv to show in the report.
        argv: Vec<String>,
    },
    /// Builtin `true`/`false`: pure status, safe anywhere in a pipeline.
    Quiet {
        /// The fixed status.
        status: i32,
        /// The argv to show in the report.
        argv: Vec<String>,
    },
}

/// An external process stage.
pub struct ExternalSpec {
    pub argv: Vec<String>,
    /// Complete child environment (PATH lookup uses this, not the parent's).
    pub env: Vec<EnvPair>,
    pub cwd: PathBuf,
    /// First stage only; later stages read the previous stage's stdout.
    pub stdin: StdinSpec,
    pub stdout_file: Option<FileSink>,
    pub stderr_file: Option<FileSink>,
    /// `2>&1`: stderr flows into the stdout stream (kernel-interleaved).
    pub stderr_to_stdout: bool,
    /// `>&2`: stdout flows into the stderr stream.
    pub stdout_to_stderr: bool,
}

/// One child-environment entry.
pub struct EnvPair {
    pub name: String,
    pub value: String,
}

/// Engine knobs for one pipeline run.
pub struct EngineConfig {
    /// Per-stream capture budget in bytes.
    pub capture_limit: usize,
    /// Also copy the final stage's stdout to the parent's stdout (live
    /// streaming in the REPL / CLI).
    pub echo_stdout: bool,
    /// Also copy every stage's stderr to the parent's stderr.
    pub echo_stderr: bool,
}

/// The forward half of a tee: at most one place bytes flow to besides the
/// capture.
enum Forward {
    Pipe(PipeWriter),
    File(File),
    ParentStdout,
    ParentStderr,
    Discard,
}

impl Forward {
    /// Write to the forward sink; false means the sink is gone (EPIPE) and
    /// the caller must stop reading so upstream sees the close.
    fn write(&mut self, buf: &[u8]) -> bool {
        let result = match self {
            Self::Pipe(writer) => writer.write_all(buf),
            Self::File(file) => file.write_all(buf),
            Self::ParentStdout => {
                let stdout = std::io::stdout();
                let mut lock = stdout.lock();
                lock.write_all(buf).and_then(|()| lock.flush())
            }
            Self::ParentStderr => {
                let stderr = std::io::stderr();
                let mut lock = stderr.lock();
                lock.write_all(buf).and_then(|()| lock.flush())
            }
            Self::Discard => Ok(()),
        };
        result.is_ok()
    }
}

/// Either a live tee thread or a capture that is already complete
/// (builtins, spawn failures, merged streams).
enum StageStream {
    Tee(JoinHandle<Capture>),
    Ready(Capture),
}

impl StageStream {
    fn spawn(mut reader: PipeReader, mut forward: Forward, limit: usize) -> Self {
        let handle = std::thread::spawn(move || {
            let mut capture = Capture::new(limit);
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        capture.write(&buf[..n]);
                        if !forward.write(&buf[..n]) {
                            // Downstream hung up: stop reading entirely so
                            // the upstream process gets SIGPIPE, matching
                            // bash. The capture keeps what already flowed.
                            break;
                        }
                    }
                }
            }
            capture
        });
        Self::Tee(handle)
    }

    fn finish(self) -> Capture {
        match self {
            Self::Tee(handle) => handle.join().unwrap_or_else(|_| Capture::new(0)),
            Self::Ready(capture) => capture,
        }
    }
}

/// A stage in flight.
struct Running {
    argv: Vec<String>,
    builtin: bool,
    /// `None` for builtins and stages that failed to spawn.
    child: Option<Child>,
    /// Status used when there is no child (builtin / spawn failure).
    fixed_status: i32,
    stdout: StageStream,
    stderr: StageStream,
    stderr_merged: bool,
    spawned_at: Instant,
    started_at_ms: u64,
}

fn open_sink(sink: &FileSink) -> Result<File, Error> {
    OpenOptions::new()
        .create(true)
        .append(sink.append)
        .write(true)
        .truncate(!sink.append)
        .open(&sink.path)
        .context(RedirectSnafu {
            path: sink.path.display().to_string(),
        })
}

/// Run one pipeline to completion. `first_stage_index` numbers the stages
/// across the whole run for `${o[N][K]}` addressing.
pub fn run_pipeline(
    stages: Vec<StageSpec>,
    config: &EngineConfig,
    first_stage_index: usize,
) -> Result<PipelineRun, Error> {
    let count = stages.len();
    let mut running: Vec<Running> = Vec::with_capacity(count);
    // Reader for the next stage's stdin, produced by the previous iteration.
    let mut next_stdin: Option<PipeReader> = None;

    for (position, spec) in stages.into_iter().enumerate() {
        let is_last = position + 1 == count;
        let stdin_reader = next_stdin.take();
        let mut edge_writer = if is_last {
            None
        } else {
            let (reader, writer) = std::io::pipe().context(IoSnafu)?;
            next_stdin = Some(reader);
            Some(writer)
        };
        let default_stdout_forward = |edge_writer: &mut Option<PipeWriter>| {
            edge_writer.take().map_or(
                if config.echo_stdout && is_last {
                    Forward::ParentStdout
                } else {
                    Forward::Discard
                },
                Forward::Pipe,
            )
        };
        let stage = match spec {
            StageSpec::Echo { text, argv } => {
                let mut forward = default_stdout_forward(&mut edge_writer);
                let mut capture = Capture::new(config.capture_limit);
                capture.write(&text);
                forward.write(&text);
                Running {
                    argv,
                    builtin: true,
                    child: None,
                    fixed_status: 0,
                    stdout: StageStream::Ready(capture),
                    stderr: StageStream::Ready(Capture::new(0)),
                    stderr_merged: false,
                    spawned_at: Instant::now(),
                    started_at_ms: crate::report::now_ms(),
                }
            }
            StageSpec::Quiet { status, argv } => Running {
                argv,
                builtin: true,
                child: None,
                fixed_status: status,
                stdout: StageStream::Ready(Capture::new(0)),
                stderr: StageStream::Ready(Capture::new(0)),
                stderr_merged: false,
                spawned_at: Instant::now(),
                started_at_ms: crate::report::now_ms(),
            },
            StageSpec::External(spec) => spawn_external(
                *spec,
                stdin_reader,
                &mut edge_writer,
                is_last,
                config,
            )?,
        };
        drop(edge_writer);
        running.push(stage);
    }

    let mut result_stages = Vec::with_capacity(count);
    for (position, stage) in running.into_iter().enumerate() {
        let wait = match stage.child.as_ref() {
            // Already reaped by wait4; dropping the Child afterwards is
            // fine (std's Drop neither waits nor kills).
            Some(child) => wait4_child(child)?,
            None => WaitOutcome {
                status: stage.fixed_status,
                user_ms: 0,
                sys_ms: 0,
            },
        };
        let stdout = stage.stdout.finish();
        let stderr = stage.stderr.finish();
        let duration = stage.spawned_at.elapsed();
        result_stages.push(Stage {
            index: first_stage_index + position,
            argv: stage.argv,
            builtin: stage.builtin,
            status: wait.status,
            started_at_ms: stage.started_at_ms,
            ended_at_ms: crate::report::now_ms(),
            duration_ms: crate::report::duration_millis(duration),
            user_ms: wait.user_ms,
            sys_ms: wait.sys_ms,
            stdout,
            stderr,
            stderr_merged: stage.stderr_merged,
        });
    }

    let status = pipefail_status(&result_stages);
    Ok(PipelineRun {
        stages: result_stages,
        status,
    })
}

/// Pipefail, except a stage killed by SIGPIPE (141): that is the normal
/// fate of an upstream whose reader finished (`yes | head`), and calling
/// it a failure would fail almost every early-exiting pipeline.
fn pipefail_status(stages: &[Stage]) -> i32 {
    stages
        .iter()
        .rev()
        .map(|stage| stage.status)
        .find(|status| *status != 0 && *status != 128 + libc::SIGPIPE)
        .unwrap_or(0)
}

fn spawn_external(
    spec: ExternalSpec,
    stdin_reader: Option<PipeReader>,
    edge_writer: &mut Option<PipeWriter>,
    is_last: bool,
    config: &EngineConfig,
) -> Result<Running, Error> {
    let mut command = Command::new(&spec.argv[0]);
    command.args(&spec.argv[1..]);
    command.env_clear();
    for pair in &spec.env {
        command.env(&pair.name, &pair.value);
    }
    command.current_dir(&spec.cwd);
    restore_default_signals(&mut command);

    // stdin
    match stdin_reader {
        Some(reader) => {
            command.stdin(Stdio::from(reader));
        }
        None => match &spec.stdin {
            StdinSpec::Null => {
                command.stdin(Stdio::null());
            }
            StdinSpec::File(path) => {
                let file = File::open(path).context(RedirectSnafu {
                    path: path.display().to_string(),
                })?;
                command.stdin(Stdio::from(file));
            }
        },
    }

    // `&> file` writes one shared file; detect it so truncation happens once.
    let shared_both = match (&spec.stdout_file, &spec.stderr_file) {
        (Some(out), Some(err)) if out.path == err.path && out.append == err.append => {
            Some(open_sink(out)?)
        }
        _ => None,
    };

    // stdout / stderr plumbing. Each stream is either a pipe back to us
    // (tee'd to capture + forward) or, for the fd-duplication forms, a clone
    // of the sibling stream's writer so the kernel interleaves exactly as
    // bash would.
    let (out_reader, out_writer) = std::io::pipe().context(IoSnafu)?;
    let (err_reader, err_writer) = std::io::pipe().context(IoSnafu)?;
    if spec.stdout_to_stderr {
        command.stdout(Stdio::from(err_writer.try_clone().context(IoSnafu)?));
    } else {
        command.stdout(Stdio::from(out_writer.try_clone().context(IoSnafu)?));
    }
    if spec.stderr_to_stdout {
        command.stderr(Stdio::from(out_writer.try_clone().context(IoSnafu)?));
    } else {
        command.stderr(Stdio::from(err_writer.try_clone().context(IoSnafu)?));
    }
    drop(out_writer);
    drop(err_writer);

    let mut shared_err: Option<File> = None;
    let stdout_forward = if let Some(file) = shared_both {
        let clone = file.try_clone().context(IoSnafu)?;
        shared_err = Some(file);
        Forward::File(clone)
    } else if let Some(sink) = &spec.stdout_file {
        Forward::File(open_sink(sink)?)
    } else if let Some(writer) = edge_writer.take() {
        Forward::Pipe(writer)
    } else if config.echo_stdout && is_last {
        Forward::ParentStdout
    } else {
        Forward::Discard
    };
    let stderr_forward = if let Some(file) = shared_err {
        Forward::File(file)
    } else if let Some(sink) = &spec.stderr_file {
        Forward::File(open_sink(sink)?)
    } else if config.echo_stderr {
        Forward::ParentStderr
    } else {
        Forward::Discard
    };

    let spawned_at = Instant::now();
    match command.spawn() {
        Ok(child) => Ok(Running {
            argv: spec.argv,
            builtin: false,
            child: Some(child),
            fixed_status: 0,
            stdout: StageStream::spawn(out_reader, stdout_forward, config.capture_limit),
            stderr: StageStream::spawn(err_reader, stderr_forward, config.capture_limit),
            stderr_merged: spec.stderr_to_stdout,
            spawned_at,
            started_at_ms: crate::report::now_ms(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut stderr = Capture::new(config.capture_limit);
            stderr.write(
                format!("plumb: command not found: {}\n", spec.argv[0]).as_bytes(),
            );
            Ok(Running {
                argv: spec.argv,
                builtin: false,
                child: None,
                fixed_status: 127,
                stdout: StageStream::Ready(Capture::new(0)),
                stderr: StageStream::Ready(stderr),
                stderr_merged: false,
                spawned_at,
                started_at_ms: crate::report::now_ms(),
            })
        }
        Err(error) => Err(Error::Io { source: error }),
    }
}

/// Give children the default signal dispositions. Rust ignores SIGPIPE
/// process-wide, and children must take the default or `yes | head` never
/// terminates; SIGINT likewise so terminal ctrl-c stops the pipeline, not
/// the shell that is waiting on it.
fn restore_default_signals(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: the closure runs post-fork pre-exec and only calls
    // async-signal-safe signal(2).
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            Ok(())
        });
    }
}

/// What `wait4` reports for a finished child.
struct WaitOutcome {
    status: i32,
    user_ms: u64,
    sys_ms: u64,
}

/// Timeval to whole milliseconds.
fn timeval_millis(time: libc::timeval) -> u64 {
    // Kernel-reported rusage times are non-negative; clamp defensively so a
    // hostile value reads as zero instead of panicking mid-reap. After the
    // clamp the conversion cannot fail: a non-negative signed value always
    // fits in u64.
    let seconds = u64::try_from(time.tv_sec.max(0)).expect("non-negative value fits in u64");
    let micros = u64::try_from(time.tv_usec.max(0)).expect("non-negative value fits in u64");
    seconds.saturating_mul(1000).saturating_add(micros / 1000)
}

/// Reap a child with `wait4(2)` so its rusage (user/kernel CPU time) comes
/// back with the exit status; `Child::wait` would discard it.
fn wait4_child(child: &Child) -> Result<WaitOutcome, Error> {
    let pid = i32::try_from(child.id()).map_err(|_| Error::Io {
        source: std::io::Error::other("pid out of i32 range"),
    })?;
    let mut status: libc::c_int = 0;
    // SAFETY: plain wait4 on a pid we spawned; rusage is a plain-old-data
    // out-parameter.
    let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        let reaped = unsafe { libc::wait4(pid, &raw mut status, 0, &raw mut rusage) };
        if reaped == pid {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(Error::Io { source: error });
        }
    }
    let exit = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    };
    Ok(WaitOutcome {
        status: exit,
        user_ms: timeval_millis(rusage.ru_utime),
        sys_ms: timeval_millis(rusage.ru_stime),
    })
}
