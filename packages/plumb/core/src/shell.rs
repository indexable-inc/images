//! The shell itself: shared state, evaluation, expansion, builtins, and the
//! automatic binding of every run's outputs to variables.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use plumb_syntax::{
    AndOr, Command as AstCommand, Connector, Part, PathSeg, Pipeline as AstPipeline, Program,
    RedirOp, Word, parse,
};

use crate::engine::{
    self, EngineConfig, EnvPair, ExternalSpec, FileSink, StageSpec, StdinSpec,
};
use crate::error::Error;
use crate::report::{Capture, PipelineRun, Report, Stage};

/// Shell construction knobs.
#[derive(Debug, Clone)]
pub struct Config {
    /// Per-stream capture budget in bytes (head + tail; exact byte counts
    /// are always kept).
    pub capture_limit: usize,
    /// Capture budget for `$(...)` output. Substitution output becomes
    /// argument data, so exceeding this is an error rather than a silent
    /// truncation.
    pub substitution_limit: usize,
    /// How many runs (reports + their auto-bound variables) to retain.
    pub keep_runs: usize,
    /// Stream final stdout / all stderr to the parent's stdio while running
    /// (the REPL and CLI turn this on; embedders usually want it off).
    pub echo_output: bool,
    /// Initial environment; `None` inherits the process environment.
    pub env: Option<HashMap<String, String>>,
    /// Initial working directory; `None` uses the process cwd.
    pub cwd: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            capture_limit: 256 * 1024,
            substitution_limit: 8 * 1024 * 1024,
            keep_runs: 64,
            echo_output: false,
            env: None,
            cwd: None,
        }
    }
}

/// A handle to one shell. Cheap to clone; clones share state, so concurrent
/// evals (REPL background jobs, embedder threads) see each other's
/// variables, cwd, and run history.
#[derive(Clone)]
pub struct Shell {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    state: Mutex<State>,
}

struct State {
    cwd: PathBuf,
    /// Exported: becomes child process environment.
    env: HashMap<String, String>,
    /// Shell-only variables (user assignments and auto-bound run outputs).
    vars: HashMap<String, String>,
    last_status: i32,
    next_run: u64,
    runs: VecDeque<Arc<Report>>,
    jobs: Vec<Job>,
}

struct Job {
    run_id: u64,
    handle: JoinHandle<()>,
}

/// A run evaluating on another thread (from [`Shell::eval_detached`]).
pub struct RunHandle {
    run_id: u64,
    handle: JoinHandle<Result<Report, Error>>,
}

impl RunHandle {
    /// The run id this eval will report under.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.run_id
    }

    /// Wait for the eval to finish.
    ///
    /// # Errors
    ///
    /// Propagates the eval's error; a panic on the eval thread surfaces as
    /// [`Error::Io`].
    pub fn join(self) -> Result<Report, Error> {
        self.handle.join().unwrap_or_else(|_| {
            Err(Error::Io {
                source: std::io::Error::other("eval thread panicked"),
            })
        })
    }
}

/// Per-eval context (differs between foreground evals and substitutions).
#[derive(Clone, Copy)]
struct RunCtx {
    echo_stdout: bool,
    capture_limit: usize,
}

impl Shell {
    /// Create a shell.
    ///
    /// # Errors
    ///
    /// Fails when no working directory can be determined.
    pub fn new(config: Config) -> Result<Self, Error> {
        let cwd = match &config.cwd {
            Some(cwd) => cwd.clone(),
            None => std::env::current_dir().map_err(|source| Error::Io { source })?,
        };
        let env = config
            .env
            .clone()
            .unwrap_or_else(|| std::env::vars().collect());
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    cwd,
                    env,
                    vars: HashMap::new(),
                    last_status: 0,
                    next_run: 1,
                    runs: VecDeque::new(),
                    jobs: Vec::new(),
                }),
                config,
            }),
        })
    }

    /// Evaluate `src` to completion and return the run's [`Report`].
    /// Background items (`&`) start their own runs and are listed in
    /// [`Report::background_started`].
    ///
    /// # Errors
    ///
    /// Parse errors, strictness violations (unset variable, empty glob),
    /// redirect failures, and [`Error::ExitRequested`] from the `exit`
    /// builtin. Commands exiting nonzero are not errors; read the report.
    /// Even on error the partial report is committed to [`Shell::reports`].
    pub fn eval(&self, src: &str) -> Result<Report, Error> {
        let program = parse(src)?;
        let run_id = self.alloc_run_id();
        let ctx = RunCtx {
            echo_stdout: self.inner.config.echo_output,
            capture_limit: self.inner.config.capture_limit,
        };
        self.run_program(run_id, src.to_owned(), program, ctx, true)
    }

    /// Evaluate `src` on a new thread, sharing this shell's state.
    #[must_use]
    pub fn eval_detached(&self, src: &str) -> RunHandle {
        let shell = self.clone();
        let source = src.to_owned();
        let run_id = self.alloc_run_id();
        let handle = std::thread::spawn(move || {
            let program = parse(&source)?;
            let ctx = RunCtx {
                echo_stdout: shell.inner.config.echo_output,
                capture_limit: shell.inner.config.capture_limit,
            };
            shell.run_program(run_id, source, program, ctx, true)
        });
        RunHandle { run_id, handle }
    }

    /// The report for a run id, if still retained.
    #[must_use]
    pub fn report(&self, id: u64) -> Option<Arc<Report>> {
        self.lock().runs.iter().find(|r| r.id == id).cloned()
    }

    /// All retained reports, oldest first.
    #[must_use]
    pub fn reports(&self) -> Vec<Arc<Report>> {
        self.lock().runs.iter().cloned().collect()
    }

    /// Read a variable (shell variables shadow the environment).
    #[must_use]
    pub fn var(&self, name: &str) -> Option<String> {
        let state = self.lock();
        state
            .vars
            .get(name)
            .or_else(|| state.env.get(name))
            .cloned()
    }

    /// Set a shell variable.
    pub fn set_var(&self, name: &str, value: &str) {
        self.lock().vars.insert(name.to_owned(), value.to_owned());
    }

    /// Status of the most recent pipeline.
    #[must_use]
    pub fn last_status(&self) -> i32 {
        self.lock().last_status
    }

    /// The shell's working directory.
    #[must_use]
    pub fn cwd(&self) -> PathBuf {
        self.lock().cwd.clone()
    }

    /// Number of background runs still in flight.
    #[must_use]
    pub fn background_pending(&self) -> usize {
        self.lock().jobs.iter().filter(|j| !j.handle.is_finished()).count()
    }

    /// Reap finished background runs, returning their reports.
    #[must_use]
    pub fn finished_background(&self) -> Vec<Arc<Report>> {
        let finished: Vec<Job> = {
            let mut state = self.lock();
            let (done, pending): (Vec<Job>, Vec<Job>) = state
                .jobs
                .drain(..)
                .partition(|job| job.handle.is_finished());
            state.jobs = pending;
            done
        };
        finished
            .into_iter()
            .filter_map(|job| {
                let _ = job.handle.join();
                self.report(job.run_id)
            })
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn alloc_run_id(&self) -> u64 {
        let mut state = self.lock();
        let id = state.next_run;
        state.next_run += 1;
        id
    }

    /// Run a parsed program as run `run_id`, committing the report (and its
    /// auto-bound variables) even when an error cuts the run short.
    fn run_program(
        &self,
        run_id: u64,
        source: String,
        program: Program,
        ctx: RunCtx,
        commit: bool,
    ) -> Result<Report, Error> {
        let started = Instant::now();
        let mut report = Report {
            id: run_id,
            source,
            status: 0,
            pipelines: Vec::new(),
            substitutions: Vec::new(),
            background_started: Vec::new(),
            duration_ms: 0,
            aborted: None,
        };
        let mut error = None;
        for item in program.items {
            if item.background {
                let bg_id = self.spawn_background(&report.source, item.and_or);
                report.background_started.push(bg_id);
                continue;
            }
            if let Err(e) = self.run_and_or(&item.and_or, &mut report, ctx) {
                report.aborted = Some(e.to_string());
                error = Some(e);
                break;
            }
        }
        report.status = report.pipelines.last().map_or(0, |p| p.status);
        report.duration_ms = crate::report::duration_millis(started.elapsed());
        if commit {
            self.commit(&report);
        }
        error.map_or(Ok(report), Err)
    }

    /// Start a background run for one `&` item; returns its run id.
    fn spawn_background(&self, parent_source: &str, and_or: AndOr) -> u64 {
        let bg_id = self.alloc_run_id();
        let source = and_or.span.text(parent_source).to_owned();
        let shell = self.clone();
        let ctx = RunCtx {
            echo_stdout: self.inner.config.echo_output,
            capture_limit: self.inner.config.capture_limit,
        };
        let handle = std::thread::spawn(move || {
            let program = Program {
                items: vec![plumb_syntax::Item {
                    and_or,
                    background: false,
                }],
            };
            // The report (including any abort reason) is committed; the
            // error itself has no foreground to land in.
            drop(shell.run_program(bg_id, source, program, ctx, true));
        });
        self.lock().jobs.push(Job {
            run_id: bg_id,
            handle,
        });
        bg_id
    }

    fn run_and_or(
        &self,
        and_or: &AndOr,
        report: &mut Report,
        ctx: RunCtx,
    ) -> Result<(), Error> {
        self.run_pipeline_ast(&and_or.first, report, ctx)?;
        let mut status = report.pipelines.last().map_or(0, |p| p.status);
        for tail in &and_or.rest {
            let run = match tail.connector {
                Connector::And => status == 0,
                Connector::Or => status != 0,
            };
            if run {
                self.run_pipeline_ast(&tail.pipeline, report, ctx)?;
                status = report.pipelines.last().map_or(0, |p| p.status);
            }
        }
        Ok(())
    }
}

/// State-mutating builtins refuse to be pipeline stages (bash would run
/// them in a subshell and silently discard their effect).
const STATE_BUILTINS: &[&str] = &["cd", "export", "unset", "exit", "wait"];

impl Shell {
    fn run_pipeline_ast(
        &self,
        pipeline: &AstPipeline,
        report: &mut Report,
        ctx: RunCtx,
    ) -> Result<(), Error> {
        let first_index = report.stages().count();
        let solo = pipeline.commands.len() == 1;

        // Bare assignment: `X=1` (no argv words).
        if let [command] = pipeline.commands.as_slice()
            && command.words.is_empty()
        {
            return self.run_bare_assignment(command, report, ctx, first_index);
        }

        let mut specs = Vec::with_capacity(pipeline.commands.len());
        for command in &pipeline.commands {
            if command.words.is_empty() {
                return Err(Error::BuiltinInPipeline {
                    name: "assignment".to_owned(),
                });
            }
            let argv = self.expand_words(&command.words, report, ctx)?;
            let name = argv[0].as_str();
            if STATE_BUILTINS.contains(&name) {
                if !solo {
                    return Err(Error::BuiltinInPipeline {
                        name: name.to_owned(),
                    });
                }
                if !command.redirects.is_empty() {
                    return Err(Error::BuiltinUsage {
                        name: name.to_owned(),
                        message: "redirections are not supported on builtins".to_owned(),
                    });
                }
                return self.run_state_builtin(&argv, report, ctx, first_index);
            }
            let spec = if command.redirects.is_empty() && name == "echo" {
                echo_spec(argv)
            } else if command.redirects.is_empty() && (name == "true" || name == "false") {
                StageSpec::Quiet {
                    status: i32::from(name == "false"),
                    argv,
                }
            } else {
                self.external_spec(command, argv, report, ctx)?
            };
            specs.push(spec);
        }

        let engine_config = EngineConfig {
            capture_limit: ctx.capture_limit,
            echo_stdout: ctx.echo_stdout,
            echo_stderr: self.inner.config.echo_output,
        };
        let run = engine::run_pipeline(specs, &engine_config, first_index)?;
        self.lock().last_status = run.status;
        report.pipelines.push(run);
        Ok(())
    }

    fn run_bare_assignment(
        &self,
        command: &AstCommand,
        report: &mut Report,
        ctx: RunCtx,
        first_index: usize,
    ) -> Result<(), Error> {
        let mut shown = Vec::with_capacity(command.assigns.len() + 1);
        shown.push("set".to_owned());
        for assign in &command.assigns {
            let value = self.expand_single(&assign.value, report, ctx)?;
            self.assign_var(&assign.name, &value);
            shown.push(format!("{}={value}", assign.name));
        }
        self.push_builtin_pipeline(report, first_index, shown, 0, None, ctx);
        Ok(())
    }

    /// Record a builtin's execution as a one-stage pipeline in the report.
    fn push_builtin_pipeline(
        &self,
        report: &mut Report,
        first_index: usize,
        argv: Vec<String>,
        status: i32,
        stderr_message: Option<String>,
        ctx: RunCtx,
    ) {
        let mut stderr = Capture::new(ctx.capture_limit);
        if let Some(message) = stderr_message {
            stderr.write(message.as_bytes());
            if self.inner.config.echo_output {
                use std::io::Write as _;
                let _ = std::io::stderr().write_all(message.as_bytes());
            }
        }
        report.pipelines.push(PipelineRun {
            stages: vec![Stage {
                index: first_index,
                argv,
                builtin: true,
                status,
                duration_ms: 0,
                stdout: Capture::new(0),
                stderr,
                stderr_merged: false,
            }],
            status,
        });
        self.lock().last_status = status;
    }

    fn run_state_builtin(
        &self,
        argv: &[String],
        report: &mut Report,
        ctx: RunCtx,
        first_index: usize,
    ) -> Result<(), Error> {
        let rest = &argv[1..];
        let outcome = match argv[0].as_str() {
            "cd" => self.builtin_cd(rest),
            "export" => self.builtin_export(rest),
            "unset" => {
                let mut state = self.lock();
                for name in rest {
                    state.vars.remove(name);
                    state.env.remove(name);
                }
                drop(state);
                Ok(())
            }
            "exit" => {
                let code = match rest {
                    [] => self.lock().last_status,
                    [code] => code.parse().map_err(|_| Error::BuiltinUsage {
                        name: "exit".to_owned(),
                        message: format!("not a number: `{code}`"),
                    })?,
                    _ => {
                        return Err(Error::BuiltinUsage {
                            name: "exit".to_owned(),
                            message: "too many arguments".to_owned(),
                        });
                    }
                };
                return Err(Error::ExitRequested { code });
            }
            "wait" => {
                self.builtin_wait(rest)?;
                Ok(())
            }
            other => unreachable!("not a state builtin: {other}"),
        };
        match outcome {
            Ok(()) => {
                self.push_builtin_pipeline(report, first_index, argv.to_vec(), 0, None, ctx);
            }
            Err(message) => {
                self.push_builtin_pipeline(
                    report,
                    first_index,
                    argv.to_vec(),
                    1,
                    Some(message),
                    ctx,
                );
            }
        }
        Ok(())
    }

    /// `cd [dir]`; failure is a status-1 stage, not a shell error.
    fn builtin_cd(&self, args: &[String]) -> Result<(), String> {
        let target = match args {
            [] => self
                .var("HOME")
                .ok_or_else(|| "cd: HOME is not set\n".to_owned())?,
            [dir] => dir.clone(),
            _ => return Err("cd: too many arguments\n".to_owned()),
        };
        let cwd = self.lock().cwd.clone();
        let joined = if Path::new(&target).is_absolute() {
            PathBuf::from(&target)
        } else {
            cwd.join(&target)
        };
        let resolved = joined
            .canonicalize()
            .map_err(|error| format!("cd: {target}: {error}\n"))?;
        if !resolved.is_dir() {
            return Err(format!("cd: {target}: not a directory\n"));
        }
        self.lock().cwd = resolved;
        Ok(())
    }

    fn builtin_export(&self, args: &[String]) -> Result<(), String> {
        let mut state = self.lock();
        for arg in args {
            if let Some(name_value) = arg.split_once('=') {
                let (name, value) = name_value;
                state.env.insert(name.to_owned(), value.to_owned());
                state.vars.remove(name);
            } else {
                let Some(value) = state.vars.remove(arg) else {
                    return Err(format!("export: {arg}: not a shell variable\n"));
                };
                state.env.insert(arg.clone(), value);
            }
        }
        drop(state);
        Ok(())
    }

    /// `wait [id ...]`: join background runs (all of them without args).
    fn builtin_wait(&self, args: &[String]) -> Result<(), Error> {
        let wanted: Vec<u64> = args
            .iter()
            .map(|arg| {
                arg.parse().map_err(|_| Error::BuiltinUsage {
                    name: "wait".to_owned(),
                    message: format!("not a run id: `{arg}`"),
                })
            })
            .collect::<Result<_, _>>()?;
        // Take the handles out of the lock before joining: the background
        // threads need the same lock to commit their reports.
        let handles: Vec<Job> = {
            let mut state = self.lock();
            if wanted.is_empty() {
                state.jobs.drain(..).collect()
            } else {
                let (take, keep): (Vec<Job>, Vec<Job>) = state
                    .jobs
                    .drain(..)
                    .partition(|job| wanted.contains(&job.run_id));
                state.jobs = keep;
                take
            }
        };
        for job in handles {
            let _ = job.handle.join();
        }
        Ok(())
    }

    fn assign_var(&self, name: &str, value: &str) {
        let mut state = self.lock();
        if state.env.contains_key(name) {
            // Assigning to an exported variable keeps it exported.
            state.env.insert(name.to_owned(), value.to_owned());
        } else {
            state.vars.insert(name.to_owned(), value.to_owned());
        }
    }
}

/// Builtin `echo` (`-n` suppresses the newline; everything else is literal,
/// like POSIX echo).
fn echo_spec(argv: Vec<String>) -> StageSpec {
    let mut rest = argv[1..].iter().peekable();
    let newline = if rest.peek().is_some_and(|arg| *arg == "-n") {
        rest.next();
        false
    } else {
        true
    };
    let mut text = rest.cloned().collect::<Vec<String>>().join(" ").into_bytes();
    if newline {
        text.push(b'\n');
    }
    StageSpec::Echo { text, argv }
}

impl Shell {
    /// Build the engine spec for one external command.
    fn external_spec(
        &self,
        command: &AstCommand,
        argv: Vec<String>,
        report: &mut Report,
        ctx: RunCtx,
    ) -> Result<StageSpec, Error> {
        let mut env: Vec<EnvPair> = {
            let state = self.lock();
            state
                .env
                .iter()
                .map(|(name, value)| EnvPair {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect()
        };
        for assign in &command.assigns {
            let value = self.expand_single(&assign.value, report, ctx)?;
            env.push(EnvPair {
                name: assign.name.clone(),
                value,
            });
        }

        let mut stdin = StdinSpec::Null;
        let mut stdout_file = None;
        let mut stderr_file = None;
        let mut stderr_to_stdout = false;
        let mut stdout_to_stderr = false;
        for redirect in &command.redirects {
            let target = match &redirect.target {
                Some(word) => Some(self.expand_single(word, report, ctx)?),
                None => None,
            };
            let sink = |append: bool| {
                target.clone().map(|path| FileSink {
                    path: PathBuf::from(path),
                    append,
                })
            };
            match redirect.op {
                RedirOp::In => {
                    stdin = StdinSpec::File(PathBuf::from(target.unwrap_or_default()));
                }
                RedirOp::OutTrunc => stdout_file = sink(false),
                RedirOp::OutAppend => stdout_file = sink(true),
                RedirOp::ErrTrunc => stderr_file = sink(false),
                RedirOp::ErrAppend => stderr_file = sink(true),
                RedirOp::BothTrunc => {
                    stdout_file = sink(false);
                    stderr_file = sink(false);
                }
                RedirOp::BothAppend => {
                    stdout_file = sink(true);
                    stderr_file = sink(true);
                }
                RedirOp::ErrToOut => stderr_to_stdout = true,
                RedirOp::OutToErr => stdout_to_stderr = true,
            }
        }
        if stderr_to_stdout && stdout_to_stderr {
            return Err(Error::BuiltinUsage {
                name: "redirection".to_owned(),
                message: "`2>&1` combined with `>&2` swaps nothing; pick one".to_owned(),
            });
        }
        // Relative redirect targets are cwd-relative.
        let cwd = self.lock().cwd.clone();
        let absolutize = |sink: Option<FileSink>| {
            sink.map(|sink| FileSink {
                path: if sink.path.is_absolute() {
                    sink.path
                } else {
                    cwd.join(sink.path)
                },
                append: sink.append,
            })
        };
        let stdin = match stdin {
            StdinSpec::File(path) if !path.is_absolute() => StdinSpec::File(cwd.join(path)),
            other => other,
        };
        let stdout_file = absolutize(stdout_file);
        let stderr_file = absolutize(stderr_file);
        Ok(StageSpec::External(Box::new(ExternalSpec {
            argv,
            env,
            cwd,
            stdin,
            stdout_file,
            stderr_file,
            stderr_to_stdout,
            stdout_to_stderr,
        })))
    }

    /// Expand argv words: variable and command substitution, then glob.
    fn expand_words(
        &self,
        words: &[Word],
        report: &mut Report,
        ctx: RunCtx,
    ) -> Result<Vec<String>, Error> {
        let mut argv = Vec::with_capacity(words.len());
        for word in words {
            let expanded = self.expand_parts(word, report, ctx)?;
            if expanded.has_glob {
                let cwd = self.lock().cwd.clone();
                argv.extend(glob_word(&expanded, &cwd)?);
            } else {
                argv.push(expanded.text);
            }
        }
        Ok(argv)
    }

    /// Expand a word to exactly one string (assignments, redirect targets):
    /// no globbing, no splitting.
    fn expand_single(
        &self,
        word: &Word,
        report: &mut Report,
        ctx: RunCtx,
    ) -> Result<String, Error> {
        Ok(self.expand_parts(word, report, ctx)?.text)
    }

    fn expand_parts(
        &self,
        word: &Word,
        report: &mut Report,
        ctx: RunCtx,
    ) -> Result<Expanded, Error> {
        let mut expanded = Expanded {
            text: String::new(),
            pattern: String::new(),
            has_glob: false,
        };
        for part in &word.parts {
            match part {
                Part::Text { text, quoted } => {
                    expanded.text.push_str(text);
                    if *quoted {
                        expanded.pattern.push_str(&glob::Pattern::escape(text));
                    } else {
                        expanded.pattern.push_str(text);
                        if text.contains(['*', '?', '[']) {
                            expanded.has_glob = true;
                        }
                    }
                }
                Part::Var { name, path, .. } => {
                    let value = if !path.is_empty() {
                        self.run_ref(name, path, report)?
                    } else if name == "?" {
                        self.lock().last_status.to_string()
                    } else {
                        self.var(name).ok_or_else(|| Error::UnsetVar {
                            name: name.clone(),
                        })?
                    };
                    expanded.text.push_str(&value);
                    // Expansion results are data, never glob patterns.
                    expanded.pattern.push_str(&glob::Pattern::escape(&value));
                }
                Part::CommandSub { program, span } => {
                    let value = self.run_substitution(program, span, report, ctx)?;
                    expanded.text.push_str(&value);
                    expanded.pattern.push_str(&glob::Pattern::escape(&value));
                }
            }
        }
        Ok(expanded)
    }

    /// Run a `$(...)` substitution: a nested run whose report lands in
    /// `report.substitutions` (id 0: substitutions are not addressable
    /// runs). Its final stdout, with trailing newlines trimmed, is the
    /// value.
    fn run_substitution(
        &self,
        program: &Program,
        span: &plumb_syntax::Span,
        report: &mut Report,
        ctx: RunCtx,
    ) -> Result<String, Error> {
        let source = span.text(&report.source).to_owned();
        let sub_ctx = RunCtx {
            echo_stdout: false,
            capture_limit: self.inner.config.substitution_limit,
        };
        let sub_report = self.run_program(0, source, program.clone(), sub_ctx, false)?;
        let truncated = sub_report
            .pipelines
            .last()
            .and_then(|p| p.stages.last())
            .is_some_and(|stage| stage.stdout.truncated());
        let value = sub_report.output();
        report.substitutions.push(sub_report);
        if truncated {
            return Err(Error::SubstitutionOverflow {
                limit: self.inner.config.substitution_limit,
            });
        }
        let _ = ctx;
        Ok(value.trim_end_matches('\n').to_owned())
    }

    /// Resolve a run reference: the terse `${o[run]}` / `${o[run][stage]}`
    /// shorthands and the structured `${runs[run]...}` paths whose field
    /// names mirror the report JSON. A non-negative run index is a run id;
    /// a negative one counts back from the latest (`-1` = latest). The run
    /// being evaluated participates once it has stages, so
    /// `a | b; use ${o[1][0]}` works within one eval. Stage indexes are the
    /// numbers shown in summaries, negative counting from the last stage.
    fn run_ref(&self, name: &str, path: &[PathSeg], current: &Report) -> Result<String, Error> {
        let shorthand = matches!(name, "o" | "e" | "s");
        if !shorthand && name != "runs" {
            return Err(Error::RunRef {
                message: format!(
                    "`{name}` is not structured; use ${{o[..]}}, ${{e[..]}}, ${{s[..]}} or ${{runs[..]}} paths"
                ),
            });
        }
        let Some(PathSeg::Index(run_index)) = path.first() else {
            return Err(Error::RunRef {
                message: "select a run first: ${runs[7]...} or ${runs[-1]...}".to_owned(),
            });
        };
        let retained: Vec<Arc<Report>> = {
            let state = self.lock();
            state.runs.iter().cloned().collect()
        };
        let mut candidates: Vec<&Report> = retained.iter().map(Arc::as_ref).collect();
        if current.stages().next().is_some() {
            candidates.push(current);
        }
        let run = if *run_index < 0 {
            usize::try_from(-(run_index + 1))
                .ok()
                .and_then(|back| candidates.len().checked_sub(back + 1))
                .and_then(|position| candidates.get(position))
        } else {
            candidates
                .iter()
                .find(|report| i64::try_from(report.id) == Ok(*run_index))
        };
        let Some(run) = run else {
            return Err(Error::RunRef {
                message: format!("run {run_index} is not retained"),
            });
        };
        let stages: Vec<&Stage> = run.stages().collect();
        if shorthand {
            let stage_index = match &path[1..] {
                [] => None,
                [PathSeg::Index(stage_index)] => Some(*stage_index),
                _ => {
                    return Err(Error::RunRef {
                        message: format!(
                            "the `{name}` shorthand takes indexes only: ${{{name}[7]}} or ${{{name}[7][0]}}"
                        ),
                    });
                }
            };
            let stage = select_stage(&stages, stage_index).ok_or_else(|| Error::RunRef {
                message: format!("run {} has no stage {:?}", run.id, stage_index),
            })?;
            let value = match (name, stage_index) {
                ("s", None) => pipeline_status(run).to_string(),
                ("s", Some(_)) => stage.status.to_string(),
                ("o", _) => trimmed(&stage.stdout.render()),
                (_, _) => trimmed(&stage.stderr.render()),
            };
            return Ok(value);
        }
        match &path[1..] {
            [] => Err(Error::RunRef {
                message: "pick a run field: .output, .stderr, .status, .id, .source, .duration_ms, or .stages[K].<field>"
                    .to_owned(),
            }),
            [PathSeg::Field(field)] if field != "stages" => run_leaf(run, &stages, field),
            [PathSeg::Field(stages_field), PathSeg::Index(stage_index), PathSeg::Field(field)]
                if stages_field == "stages" =>
            {
                let stage =
                    select_stage(&stages, Some(*stage_index)).ok_or_else(|| Error::RunRef {
                        message: format!("run {} has no stage {stage_index}", run.id),
                    })?;
                stage_leaf(stage, field)
            }
            _ => Err(Error::RunRef {
                message: "unsupported path; shapes: ${runs[N].output} or ${runs[N].stages[K].stdout}"
                    .to_owned(),
            }),
        }
    }

    /// The run history itself is the addressable surface: `${o[N]}` /
    /// `${e[N]}` / `${s[N]}` (and `[N][K]` per stage) resolve against it in
    /// [`Self::run_ref`]. Only the unnumbered `$o` / `$e` / `$s` aliases
    /// for the most recently finished run live in the variable map.
    fn commit(&self, report: &Report) {
        let arc = Arc::new(report.clone());
        let out = trimmed(&arc.output());
        let err = trimmed(
            &arc.pipelines
                .last()
                .and_then(|p| p.stages.last())
                .map(|s| s.stderr.render())
                .unwrap_or_default(),
        );
        let status = arc.status.to_string();
        let mut state = self.lock();
        state.vars.insert("o".to_owned(), out);
        state.vars.insert("e".to_owned(), err);
        state.vars.insert("s".to_owned(), status);
        state.runs.push_back(arc);
        while state.runs.len() > self.inner.config.keep_runs {
            state.runs.pop_front();
        }
        drop(state);
    }
}

/// Select a stage by summary index (negative from the end); `None` picks
/// the final stage.
fn select_stage<'stages>(
    stages: &[&'stages Stage],
    selector: Option<i64>,
) -> Option<&'stages Stage> {
    match selector {
        None => stages.last().copied(),
        Some(index) if index < 0 => usize::try_from(-(index + 1))
            .ok()
            .and_then(|back| stages.len().checked_sub(back + 1))
            .and_then(|position| stages.get(position).copied()),
        Some(index) => stages
            .iter()
            .find(|stage| i64::try_from(stage.index) == Ok(index))
            .copied(),
    }
}

/// The honest status for finished and in-progress runs alike: the last
/// pipeline's status.
fn pipeline_status(run: &Report) -> i32 {
    run.pipelines
        .last()
        .map_or(run.status, |pipeline| pipeline.status)
}

/// A `${runs[N].<field>}` leaf.
fn run_leaf(run: &Report, stages: &[&Stage], field: &str) -> Result<String, Error> {
    let value = match field {
        "output" => trimmed(&run.output()),
        "stderr" => stages
            .last()
            .map(|stage| trimmed(&stage.stderr.render()))
            .unwrap_or_default(),
        "status" => pipeline_status(run).to_string(),
        "id" => run.id.to_string(),
        "source" => run.source.clone(),
        "duration_ms" => run.duration_ms.to_string(),
        _ => {
            return Err(Error::RunRef {
                message: format!(
                    "unknown run field `{field}` (have: output, stderr, status, id, source, duration_ms, stages)"
                ),
            });
        }
    };
    Ok(value)
}

/// A `${runs[N].stages[K].<field>}` leaf.
fn stage_leaf(stage: &Stage, field: &str) -> Result<String, Error> {
    let value = match field {
        "stdout" => trimmed(&stage.stdout.render()),
        "stderr" => trimmed(&stage.stderr.render()),
        "status" => stage.status.to_string(),
        "argv" => stage.argv.join(" "),
        "duration_ms" => stage.duration_ms.to_string(),
        "index" => stage.index.to_string(),
        "stdout_bytes" => stage.stdout.total_bytes().to_string(),
        "stderr_bytes" => stage.stderr.total_bytes().to_string(),
        _ => {
            return Err(Error::RunRef {
                message: format!(
                    "unknown stage field `{field}` (have: stdout, stderr, status, argv, duration_ms, index, stdout_bytes, stderr_bytes)"
                ),
            });
        }
    };
    Ok(value)
}

/// Capture text with trailing newlines trimmed, the ergonomic form for
/// variable-shaped reuse (mirrors `$(...)` trimming).
fn trimmed(text: &str) -> String {
    text.trim_end_matches('\n').to_owned()
}

/// A word after variable/substitution expansion, with the parallel glob
/// pattern (expansion results escaped) when any unquoted metacharacter
/// appeared.
struct Expanded {
    text: String,
    pattern: String,
    has_glob: bool,
}

/// Expand a glob word against `cwd`. No match is an error (failglob); a
/// syntactically invalid pattern (a lone `[`) falls back to the literal
/// text so `[ -f x ]` keeps working.
fn glob_word(expanded: &Expanded, cwd: &Path) -> Result<Vec<String>, Error> {
    let anchored = if Path::new(&expanded.pattern).is_absolute() {
        expanded.pattern.clone()
    } else {
        format!("{}/{}", cwd.display(), expanded.pattern)
    };
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        // `*` must not match dotfiles, same as bash.
        require_literal_leading_dot: true,
    };
    let Ok(paths) = glob::glob_with(&anchored, options) else {
        return Ok(vec![expanded.text.clone()]);
    };
    let matches: Vec<String> = paths
        .filter_map(Result::ok)
        .map(|path| {
            path.strip_prefix(cwd).map_or_else(
                |_| path.display().to_string(),
                |relative| relative.display().to_string(),
            )
        })
        .collect();
    if matches.is_empty() {
        return Err(Error::GlobNoMatch {
            pattern: expanded.text.clone(),
        });
    }
    Ok(matches)
}
