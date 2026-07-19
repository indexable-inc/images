//! Elixir binding for the local `tui` PTY driver: spawn a real terminal
//! program (an editor, a pager, a REPL) from the BEAM, send it keystrokes,
//! and read back the VT100-rendered screen. The Elixir twin of the Python
//! kernel's `tui` module, scoped to the driving slice: spawn, send,
//! snapshot, wait, kill.
//!
//! Terminals cross the boundary as string ids rather than BEAM resources,
//! on purpose: `TuiManager::spawn` legitimately blocks (PTY open, fork, a
//! first-paint wait), unibind constructors cannot be `blocking`, and an id
//! survives a workspace checkpoint where an opaque reference would not. The
//! process-global manager keeps every spawned terminal alive until `close`
//! removes it, exactly like the Python binding's `Tui.list_all()` registry.

/// The exported boundary. The module name names the generated Elixir
/// namespace (`TuiEx`) and the OTP app (`:tui_ex`).
#[unibind::export(backends(ex))]
mod _tui_ex {
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    /// How often `wait_for` re-reads the screen while polling.
    const POLL_INTERVAL: Duration = Duration::from_millis(25);

    /// Boundary failures. A child merely exiting nonzero is data
    /// (`exit_code`), never an error.
    #[unibind::error]
    #[derive(Debug)]
    pub enum TuiError {
        /// The command could not be spawned onto a PTY.
        Spawn {
            /// What failed to start.
            message: String,
        },
        /// No live terminal has this id (never spawned, or `close`d).
        NotFound {
            /// The unknown id.
            message: String,
        },
        /// Talking to the terminal actor failed.
        Io {
            /// The underlying failure.
            message: String,
        },
        /// `send_key` got a name outside the key table.
        BadKey {
            /// The unknown key name.
            message: String,
        },
        /// `wait_for`/`wait` hit its deadline.
        Timeout {
            /// What was awaited.
            message: String,
        },
    }

    impl std::fmt::Display for TuiError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Spawn { message }
                | Self::NotFound { message }
                | Self::Io { message }
                | Self::BadKey { message }
                | Self::Timeout { message } => write!(formatter, "{message}"),
            }
        }
    }

    impl std::error::Error for TuiError {}

    /// The BEAM's main VM process ignores SIGCHLD (ports fork from
    /// erl_child_setup, so the VM expects to own no children), and SIG_IGN
    /// auto-reaps our PTY children before the manager's reaper can collect
    /// their exit statuses. Restore the default disposition once, before
    /// the first spawn; mirrors packages/plumb/ex.
    fn ensure_sigchld_default() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // SAFETY: signal(2) with a standard disposition; no handler
            // code runs.
            unsafe {
                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            }
        });
    }

    /// One manager (and one tokio runtime) per BEAM node, alive for the
    /// process lifetime, exactly like tui-py's `global_manager`.
    fn manager() -> &'static tui::TuiManager {
        static MANAGER: OnceLock<tui::TuiManager> = OnceLock::new();
        MANAGER.get_or_init(|| {
            ensure_sigchld_default();
            tui::TuiManager::new()
        })
    }

    fn instance(id: &str) -> Result<tui::TuiInstance, TuiError> {
        manager()
            .list()
            .into_iter()
            .find(|handle| handle.id.to_string() == id)
            .ok_or_else(|| TuiError::NotFound {
                message: format!("no live terminal with id {id:?}"),
            })
    }

    fn io_error(error: &tui::Error) -> TuiError {
        TuiError::Io {
            message: error.to_string(),
        }
    }

    /// The ANSI byte sequence for a named key, mirroring the Python
    /// binding's `tui.Key` table plus `ctrl+<letter>` / `alt+<char>`
    /// chords.
    fn key_bytes(name: &str) -> Result<String, TuiError> {
        let normalized = name.trim().to_ascii_lowercase();
        if let Some(letter) = normalized.strip_prefix("ctrl+") {
            let mut chars = letter.chars();
            if let (Some(ch @ 'a'..='z'), None) = (chars.next(), chars.next()) {
                return Ok(char::from(ch as u8 - b'a' + 1).to_string());
            }
            return Err(TuiError::BadKey {
                message: format!("ctrl+ expects one ASCII letter a-z, got {name:?}"),
            });
        }
        if let Some(rest) = normalized.strip_prefix("alt+") {
            let mut chars = rest.chars();
            if let (Some(ch), None) = (chars.next(), chars.next()) {
                return Ok(format!("\x1b{ch}"));
            }
            return Err(TuiError::BadKey {
                message: format!("alt+ expects a single character, got {name:?}"),
            });
        }
        let bytes = match normalized.as_str() {
            "enter" => "\r",
            "tab" => "\t",
            "backtab" => "\x1b[Z",
            "esc" | "escape" => "\x1b",
            "backspace" => "\x7f",
            "delete" => "\x1b[3~",
            "space" => " ",
            "up" => "\x1b[A",
            "down" => "\x1b[B",
            "right" => "\x1b[C",
            "left" => "\x1b[D",
            "home" => "\x1b[H",
            "end" => "\x1b[F",
            "page_up" => "\x1b[5~",
            "page_down" => "\x1b[6~",
            "f1" => "\x1bOP",
            "f2" => "\x1bOQ",
            "f3" => "\x1bOR",
            "f4" => "\x1bOS",
            "f5" => "\x1b[15~",
            "f6" => "\x1b[17~",
            "f7" => "\x1b[18~",
            "f8" => "\x1b[19~",
            "f9" => "\x1b[20~",
            "f10" => "\x1b[21~",
            "f11" => "\x1b[23~",
            "f12" => "\x1b[24~",
            _ => {
                return Err(TuiError::BadKey {
                    message: format!("unknown key name {name:?}"),
                });
            }
        };
        Ok(bytes.to_owned())
    }

    fn viewport_text(handle: &tui::TuiInstance) -> Result<String, TuiError> {
        handle
            .read_viewport()
            .map(|lines| lines.join("\n"))
            .map_err(|error| io_error(&error))
    }

    /// Spawn `command` with `argv` on a fresh PTY sized `rows` x `cols`;
    /// returns the terminal's id. The child sees a real terminal
    /// (`TERM=xterm-256color`), so editors, pagers, and REPLs behave as
    /// they would interactively. Blocking (DirtyIo): the spawn waits
    /// briefly for the child's first paint.
    #[unibind(blocking)]
    pub fn spawn(
        command: String,
        // Not `args`: rustler's nif macro names its own argument slice
        // `args`, and a same-named parameter shadows it, breaking the
        // decode of every later parameter.
        argv: Vec<String>,
        #[unibind(default = 24)] rows: u16,
        #[unibind(default = 80)] cols: u16,
    ) -> Result<String, TuiError> {
        let config = tui::SpawnConfig {
            rows,
            cols,
            ..tui::SpawnConfig::default()
        };
        manager()
            .spawn(command, argv, config)
            .map(|handle| handle.id.to_string())
            .map_err(|error| TuiError::Spawn {
                message: error.to_string(),
            })
    }

    /// Send literal text (escape sequences included) to the terminal.
    /// While the program has DECCKM (application cursor keys) enabled, bare
    /// cursor CSIs are rewritten to their SS3 form by the driver, so arrow
    /// bytes reach full-screen programs either way.
    #[unibind(blocking)]
    pub fn send(id: String, data: String) -> Result<(), TuiError> {
        instance(&id)?
            .write(&data)
            .map_err(|error| io_error(&error))
    }

    /// Send one named key: `enter`, `esc`, `tab`, `backspace`, `delete`,
    /// arrows (`up`/`down`/`left`/`right`), `home`/`end`,
    /// `page_up`/`page_down`, `f1`..`f12`, or a `ctrl+<letter>` /
    /// `alt+<char>` chord.
    #[unibind(blocking)]
    pub fn send_key(id: String, key: String) -> Result<(), TuiError> {
        let bytes = key_bytes(&key)?;
        instance(&id)?
            .write(&bytes)
            .map_err(|error| io_error(&error))
    }

    /// The rendered screen as plain text: one line per viewport row,
    /// trailing blank rows dropped.
    #[unibind(blocking)]
    pub fn snapshot(id: String) -> Result<String, TuiError> {
        viewport_text(&instance(&id)?)
    }

    /// Poll the screen until `pattern` appears as a substring; returns the
    /// matching screen text. Times out with `TuiError::Timeout` after
    /// `timeout_ms` (call `snapshot` afterwards to see what the screen
    /// held).
    #[unibind(blocking)]
    pub fn wait_for(
        id: String,
        pattern: String,
        #[unibind(default = 5000)] timeout_ms: u64,
    ) -> Result<String, TuiError> {
        let handle = instance(&id)?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let text = viewport_text(&handle)?;
            if text.contains(&pattern) {
                return Ok(text);
            }
            if Instant::now() >= deadline {
                return Err(TuiError::Timeout {
                    message: format!("{pattern:?} did not appear within {timeout_ms}ms"),
                });
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Whether the child process is still running. Reads cached state, so
    /// it is safe on a normal scheduler.
    pub fn is_alive(id: String) -> Result<bool, TuiError> {
        Ok(instance(&id)?.is_alive())
    }

    /// The exit code: `nil` while running or after a signal kill, the code
    /// once the child exited on its own. Reads cached state.
    pub fn exit_code(id: String) -> Result<Option<i32>, TuiError> {
        Ok(match instance(&id)?.exit_state() {
            tui::ExitState::Exited(code) => code,
            tui::ExitState::Running => None,
        })
    }

    /// Block until the child exits; returns its exit code (`nil` for a
    /// signal death). Times out with `TuiError::Timeout` after
    /// `timeout_ms`.
    #[unibind(blocking)]
    pub fn wait(
        id: String,
        #[unibind(default = 5000)] timeout_ms: u64,
    ) -> Result<Option<i32>, TuiError> {
        let handle = instance(&id)?;
        match handle.wait(Some(Duration::from_millis(timeout_ms))) {
            Some(tui::ExitState::Exited(code)) => Ok(code),
            Some(tui::ExitState::Running) | None => Err(TuiError::Timeout {
                message: format!("still running after {timeout_ms}ms"),
            }),
        }
    }

    /// Resize the terminal (delivers SIGWINCH to the child).
    #[unibind(blocking)]
    pub fn resize(id: String, rows: u16, cols: u16) -> Result<(), TuiError> {
        instance(&id)?
            .resize(rows, cols)
            .map_err(|error| io_error(&error))
    }

    /// Force-terminate the child with SIGKILL. A no-op if it already
    /// exited; the terminal stays readable until `close`.
    #[unibind(blocking)]
    pub fn kill(id: String) -> Result<(), TuiError> {
        instance(&id)?.kill().map_err(|error| io_error(&error))
    }

    /// Kill the child and drop the terminal from the registry; its id stops
    /// resolving.
    #[unibind(blocking)]
    pub fn close(id: String) -> Result<(), TuiError> {
        let handle = instance(&id)?;
        handle.kill().map_err(|error| io_error(&error))?;
        drop(manager().remove(&handle.id));
        Ok(())
    }

    /// Ids of every live terminal in this process.
    pub fn list() -> Vec<String> {
        manager()
            .list()
            .into_iter()
            .map(|handle| handle.id.to_string())
            .collect()
    }
}
