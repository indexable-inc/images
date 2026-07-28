//! Elixir binding for the plumb shell (#3580): call `plumb-core` in-process
//! from the BEAM and get each run's report as JSON.
//!
//! The surface is JSON-first on purpose: `Report` is deeply nested and
//! Elixir 1.18+ decodes JSON natively, so a typed record mirror can come
//! later without breaking anyone. `eval` is `#[unibind(blocking)]`
//! (DirtyIo): a run legitimately blocks on child processes and must not sit
//! on a normal BEAM scheduler.

/// The exported boundary. The module name names the generated Elixir
/// namespace (`Plumb`) and the OTP app (`:plumb`).
#[unibind::export(backends(ex))]
mod _plumb {
    /// Boundary failures. Everything the shell itself refuses to carry
    /// forward; a command merely exiting nonzero is data in the report,
    /// never an error.
    #[unibind::error]
    #[derive(Debug)]
    pub enum PlumbError {
        /// The source is not in the plumb subset.
        Parse {
            /// The parser's diagnosis.
            message: String,
        },
        /// A strictness violation: unset variable, empty glob, builtin
        /// misuse, substitution overflow.
        Strict {
            /// What was violated.
            message: String,
        },
        /// Plumbing failed: pipes, spawn machinery, redirect targets,
        /// report serialization.
        Io {
            /// The underlying failure.
            message: String,
        },
        /// The `exit` builtin ran; the message carries the requested code.
        Exit {
            /// `exit <code>`.
            message: String,
        },
    }

    unibind_ex_runtime::message_error!(PlumbError {
        Parse,
        Strict,
        Io,
        Exit,
    });

    fn convert(error: plumb_core::Error) -> PlumbError {
        use plumb_core::Error;
        let message = error.to_string();
        match error {
            Error::Parse { .. } => PlumbError::Parse { message },
            Error::UnsetVar { .. }
            | Error::GlobNoMatch { .. }
            | Error::BuiltinInPipeline { .. }
            | Error::BuiltinUsage { .. }
            | Error::RunRef { .. }
            | Error::SubstitutionOverflow { .. } => PlumbError::Strict { message },
            Error::Redirect { .. } | Error::Io { .. } => PlumbError::Io { message },
            Error::ExitRequested { .. } => PlumbError::Exit { message },
        }
    }

    fn to_json<T: serde::Serialize>(value: &T) -> Result<String, PlumbError> {
        serde_json::to_string(value).map_err(|error| PlumbError::Io {
            message: error.to_string(),
        })
    }

    /// One shell: shared variables, cwd, and run history across evals.
    /// The BEAM collecting the resource drops the handle; background runs
    /// already started keep their own clones and finish.
    #[unibind::object]
    pub struct Shell {
        inner: plumb_core::Shell,
    }

    impl Shell {
        /// Open a shell (process env, process cwd, output captured only).
        #[unibind(constructor)]
        pub fn new() -> Result<Self, PlumbError> {
            unibind_ex_runtime::ensure_sigchld_default();
            plumb_core::Shell::new(plumb_core::Config::default())
                .map(|inner| Self { inner })
                .map_err(convert)
        }

        /// Evaluate to completion; returns the run's report as JSON.
        /// Blocking (DirtyIo): a run waits on child processes.
        #[unibind(blocking)]
        pub fn eval(&self, src: String) -> Result<String, PlumbError> {
            let report = self.inner.eval(&src).map_err(convert)?;
            to_json(&report)
        }

        /// Start a run without waiting; returns its run id. Poll `report`
        /// for the result (it appears when the run commits).
        pub fn eval_start(&self, src: String) -> u64 {
            // Dropping the handle detaches the thread; the run still
            // commits its report and variables to the shared shell.
            self.inner.eval_detached(&src).id()
        }

        /// The report of run `id` as JSON, if finished and still retained.
        pub fn report(&self, id: u64) -> Result<Option<String>, PlumbError> {
            match self.inner.report(id) {
                Some(report) => to_json(&*report).map(Some),
                None => Ok(None),
            }
        }

        /// Ids of the retained runs, oldest first.
        pub fn run_ids(&self) -> Vec<u64> {
            self.inner.reports().iter().map(|report| report.id).collect()
        }

        /// Read a variable (auto-bound run outputs included).
        pub fn var(&self, name: String) -> Option<String> {
            self.inner.var(&name)
        }

        /// Set a shell variable.
        pub fn set_var(&self, name: String, value: String) {
            self.inner.set_var(&name, &value);
        }

        /// Status of the most recent pipeline.
        pub fn last_status(&self) -> i64 {
            i64::from(self.inner.last_status())
        }

        /// The shell's working directory.
        pub fn cwd(&self) -> String {
            self.inner.cwd().display().to_string()
        }
    }

    /// One-shot: a fresh shell, one eval, the report as JSON.
    #[unibind(blocking)]
    pub fn run(src: String) -> Result<String, PlumbError> {
        let shell = Shell::new()?;
        shell.eval(src)
    }
}
