//! Python bindings for an embedded nushell engine.
//!
//! One [`Engine`] holds a persistent `EngineState` + `Stack`, so `let`
//! bindings, `def`s, and `cd` survive across `eval` calls the way they do in a
//! REPL. `eval` returns a native asyncio coroutine (bridged through
//! pyo3-async-runtimes); the synchronous nushell evaluation runs on tokio's
//! blocking pool, never on the caller's event loop.
//!
//! Cancellation: the engine's `Signals` share one `AtomicBool` with
//! [`Engine::interrupt`]; flipping it makes the evaluator stop between
//! pipeline elements, so a Python-side timeout can end a runaway pipeline
//! without killing the process. (An external command the pipeline already
//! spawned still runs to completion; nushell only checks the flag between
//! elements.)
//!
//! Values cross the boundary natively, not as JSON: date -> `datetime`
//! (normalized to UTC so a column mixes no offsets), duration -> `timedelta`,
//! filesize -> `int` bytes, binary -> `bytes`, record -> `dict`, list ->
//! `list`. The `nu` Python package turns those into polars frames.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use nu_protocol::debugger::WithoutDebug;
use nu_protocol::engine::{EngineState, Stack, StateWorkingSet};
use nu_protocol::{
    ErrorStyle, PipelineData, Record, ShellError, Signals, Span, Value,
    report_error::format_cli_error,
};
use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

create_exception!(
    _nu,
    NuError,
    pyo3::exceptions::PyException,
    "A nushell pipeline failed; the message is nushell's own rendered diagnostic."
);

/// The engine state a fresh [`Engine`] starts from: the full shell command
/// set, the host environment, and REPL-free configuration.
fn initial_engine_state() -> EngineState {
    let mut engine_state = nu_cmd_lang::create_default_context();
    engine_state = nu_command::add_shell_command_context(engine_state);
    engine_state = nu_cmd_extra::add_extra_command_context(engine_state);

    engine_state.history_enabled = false;
    engine_state.is_interactive = false;
    engine_state.is_login = false;
    // This engine lives inside an MCP server process: run_external gives an
    // external with empty pipeline input a NULL stdin only when is_mcp is set
    // (run_external.rs); otherwise the child inherits the process stdin and a
    // prompting CLI could hang on -- or consume -- the MCP stdio transport.
    engine_state.is_mcp = true;
    engine_state.generate_nu_constant();

    // Plain diagnostics: the consumer is a model reading an exception message,
    // so drop the fancy unicode/ANSI rendering at the source instead of
    // stripping escapes after the fact.
    {
        let config = Arc::make_mut(&mut engine_state.config);
        config.error_style = ErrorStyle::Plain;
    }

    // The host environment, so `$env`, externals, and path lookups behave like
    // a normal shell session. PWD seeds cwd-relative commands (`ls`, `open`).
    // GH_FORCE_TTY is the one exclusion: it makes gh render TTY-style output
    // (color, truncation) into a captured pipe, so it never crosses over.
    for (key, value) in std::env::vars() {
        if key == "GH_FORCE_TTY" {
            continue;
        }
        engine_state.add_env_var(key, Value::string(value, Span::unknown()));
    }
    // Color-free externals by default (issue #2051): the host process often
    // forces color (e.g. Claude Code exports FORCE_COLOR=1 / CLICOLOR_FORCE=1),
    // and pipeline output here is parsed rather than displayed, so inherited
    // forcing breaks `^gh ... --json | from json` with ANSI-wrapped JSON.
    // Overriding the values (not merely unsetting them) beats every CLI color
    // convention; a caller that wants ANSI back re-enables it for one call via
    // `env=` (the per-eval stack shadows these) or `with-env`.
    for (key, value) in [
        ("NO_COLOR", "1"),
        ("CLICOLOR", "0"),
        ("CLICOLOR_FORCE", "0"),
        ("FORCE_COLOR", "0"),
    ] {
        engine_state.add_env_var(key.into(), Value::string(value, Span::unknown()));
    }
    if let Ok(current_dir) = std::env::current_dir() {
        engine_state.add_env_var(
            "PWD".into(),
            Value::string(current_dir.to_string_lossy(), Span::unknown()),
        );
    }

    engine_state
}

/// The mutable half of an [`Engine`], locked for the duration of one eval.
struct EngineInner {
    engine_state: EngineState,
    stack: Stack,
}

impl EngineInner {
    /// Parse and evaluate `code` against the persistent state, returning the
    /// pipeline's collected output value. Every error path returns nushell's
    /// rendered diagnostic (span, label, help) as the message.
    fn eval(
        &mut self,
        code: &str,
        input: Option<Value>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        interrupt: &Arc<AtomicBool>,
    ) -> Result<Value, String> {
        let Self {
            engine_state,
            stack,
        } = self;

        // Each eval carries its OWN interrupt flag (see EvalHandle): installing
        // it here, under the engine lock, means an interrupt can only ever stop
        // the eval it was issued for -- a queued eval can neither erase nor
        // receive a signal aimed at the one currently running.
        engine_state.set_signals(Signals::new(Arc::clone(interrupt)));

        if let Some(dir) = cwd {
            stack.add_env_var("PWD".into(), Value::string(dir, Span::unknown()));
        }
        for (key, value) in env.into_iter().flatten() {
            stack.add_env_var(key, Value::string(value, Span::unknown()));
        }

        let block = {
            let mut working_set = StateWorkingSet::new(engine_state);
            let block = nu_parser::parse(&mut working_set, Some("nu()"), code.as_bytes(), false);
            if let Some(error) = working_set.parse_errors.first() {
                return Err(format_cli_error(
                    Some(stack),
                    &working_set,
                    error,
                    Some("nu::parser::error"),
                ));
            }
            if let Some(error) = working_set.compile_errors.first() {
                return Err(format_cli_error(
                    Some(stack),
                    &working_set,
                    error,
                    Some("nu::compile::error"),
                ));
            }
            let delta = working_set.render();
            engine_state
                .merge_delta(delta)
                .map_err(|error| render_shell_error(engine_state, stack, &error))?;
            block
        };

        let input = input.map_or_else(PipelineData::empty, |value| PipelineData::value(value, None));
        // eval_ir_block, NOT eval_block: eval_block maps a user's `exit` to
        // std::process::exit, which would take the whole embedding process
        // (the kernel) down. Here `exit` surfaces as ShellError::Exit and
        // becomes a raised NuError like any other failure.
        let executed =
            nu_engine::eval_ir_block::<WithoutDebug>(engine_state, stack, &block, input)
                .map_err(|error| render_shell_error(engine_state, stack, &error))?;
        let value = executed
            .body
            .into_value(Span::unknown())
            .map_err(|error| render_shell_error(engine_state, stack, &error))?;
        if let Value::Error { error, .. } = value {
            return Err(render_shell_error(engine_state, stack, &error));
        }
        Ok(value)
    }
}

/// Render a `ShellError` exactly the way the nushell CLI would (minus color:
/// the engine config pins the plain style).
fn render_shell_error(engine_state: &EngineState, stack: &Stack, error: &ShellError) -> String {
    let working_set = StateWorkingSet::new(engine_state);
    format_cli_error(Some(stack), &working_set, error, Some("nu::shell::error"))
}

/// Convert a nushell [`Value`] into the natural Python object.
fn value_to_py(py: Python<'_>, value: Value) -> PyResult<Py<PyAny>> {
    let object = match value {
        Value::Nothing { .. } => py.None(),
        Value::Bool { val, .. } => val.into_pyobject(py)?.to_owned().unbind().into_any(),
        Value::Int { val, .. } => val.into_pyobject(py)?.unbind().into_any(),
        Value::Float { val, .. } => val.into_pyobject(py)?.unbind().into_any(),
        Value::String { val, .. } | Value::Glob { val, .. } => {
            val.into_pyobject(py)?.unbind().into_any()
        }
        // Bytes, not a unit-carrying type: polars sums/filters plain ints.
        Value::Filesize { val, .. } => i64::from(val).into_pyobject(py)?.unbind().into_any(),
        // Nanoseconds -> timedelta (polars maps it to a Duration column).
        Value::Duration { val, .. } => TimeDelta::nanoseconds(val)
            .into_pyobject(py)?
            .unbind()
            .into_any(),
        // Normalize to UTC so a frame column never mixes fixed offsets.
        Value::Date { val, .. } => val
            .with_timezone(&Utc)
            .into_pyobject(py)?
            .unbind()
            .into_any(),
        Value::Binary { val, .. } => PyBytes::new(py, &val).unbind().into_any(),
        Value::Record { val, .. } => {
            let dict = PyDict::new(py);
            for (key, item) in val.into_owned() {
                dict.set_item(key, value_to_py(py, item)?)?;
            }
            dict.unbind().into_any()
        }
        Value::List { vals, .. } => {
            let list = PyList::empty(py);
            for item in vals {
                list.append(value_to_py(py, item)?)?;
            }
            list.unbind().into_any()
        }
        // A bounded range expands to its values; an unbounded one has no
        // finite Python shape, so refuse it rather than loop forever (the
        // range iterator itself never checks signals here).
        Value::Range { ref val, .. } => {
            const MAX_RANGE_ELEMENTS: usize = 1_000_000;
            let span = value.span();
            let list = PyList::empty(py);
            for item in val
                .into_range_iter(span, Signals::empty())
                .take(MAX_RANGE_ELEMENTS + 1)
            {
                if list.len() >= MAX_RANGE_ELEMENTS {
                    return Err(NuError::new_err(
                        "range is unbounded or has more than 1,000,000 elements; \
                         collect it in nushell first (e.g. `| first 1000`)",
                    ));
                }
                list.append(value_to_py(py, item)?)?;
            }
            list.unbind().into_any()
        }
        // An error embedded in otherwise-successful data still fails the call:
        // silently stringifying it would hide the failure in a frame cell.
        Value::Error { error, .. } => return Err(NuError::new_err(error.to_string())),
        // No natural Python shape: hand back the value's own string rendering.
        other @ (Value::Closure { .. } | Value::CellPath { .. } | Value::Custom { .. }) => other
            .to_expanded_string(", ", &nu_protocol::Config::default())
            .into_pyobject(py)?
            .unbind()
            .into_any(),
    };
    Ok(object)
}

/// Convert a Python object into a nushell [`Value`] (the `input=` direction).
fn py_to_value(object: &Bound<'_, PyAny>) -> PyResult<Value> {
    let span = Span::unknown();
    if object.is_none() {
        return Ok(Value::nothing(span));
    }
    if let Ok(val) = object.extract::<bool>() {
        return Ok(Value::bool(val, span));
    }
    // Guard ints as a TYPE, not by extraction fallthrough: a Python int past
    // i64 would otherwise fall to the f64 branch and arrive silently rounded.
    if object.is_instance_of::<pyo3::types::PyInt>() {
        return match object.extract::<i64>() {
            Ok(val) => Ok(Value::int(val, span)),
            Err(_) => Err(NuError::new_err(
                "integer out of range for a nushell int (i64); pass it as a string \
                 or a float explicitly if lossy is acceptable",
            )),
        };
    }
    if let Ok(val) = object.extract::<f64>() {
        return Ok(Value::float(val, span));
    }
    if let Ok(val) = object.extract::<DateTime<FixedOffset>>() {
        return Ok(Value::date(val, span));
    }
    // A tz-naive datetime would otherwise fall through every branch and hit
    // the generic type error; name the actual problem instead of guessing a
    // timezone (assuming UTC or local silently would corrupt data).
    if object.extract::<chrono::NaiveDateTime>().is_ok() {
        return Err(NuError::new_err(
            "naive datetime: nushell dates carry a timezone; attach one first, \
             e.g. stamp.replace(tzinfo=datetime.UTC)",
        ));
    }
    if let Ok(val) = object.extract::<TimeDelta>() {
        let nanos = val
            .num_nanoseconds()
            .ok_or_else(|| NuError::new_err("timedelta too large for a nushell duration"))?;
        return Ok(Value::duration(nanos, span));
    }
    if let Ok(val) = object.extract::<String>() {
        return Ok(Value::string(val, span));
    }
    // Real bytes objects only: extract::<Vec<u8>> would also accept ANY
    // sequence of byte-sized ints, turning a documented list input like
    // [1, 2, 3] into binary before the list branch below could see it.
    if let Ok(bytes) = object.cast::<PyBytes>() {
        return Ok(Value::binary(bytes.as_bytes().to_vec(), span));
    }
    if let Ok(bytes) = object.cast::<pyo3::types::PyByteArray>() {
        return Ok(Value::binary(bytes.to_vec(), span));
    }
    if let Ok(dict) = object.cast::<PyDict>() {
        let mut record = Record::new();
        for (key, item) in dict {
            record.push(key.extract::<String>()?, py_to_value(&item)?);
        }
        return Ok(Value::record(record, span));
    }
    if let Ok(list) = object.try_iter() {
        let mut vals = Vec::new();
        for item in list {
            vals.push(py_to_value(&item?)?);
        }
        return Ok(Value::list(vals, span));
    }
    Err(NuError::new_err(format!(
        "cannot pipe a {} into nushell; pass None/bool/int/float/str/bytes/datetime/timedelta \
         or a list/dict of those",
        object.get_type().name()?,
    )))
}

/// One eval's interrupt token, returned by [`Engine::eval`] next to the
/// awaitable. Flipping it stops THAT eval (at its next pipeline-element
/// boundary, ctrl-c semantics) and no other: an engine-wide flag could hit a
/// different eval than the one that timed out, or be erased by a queued one.
#[pyclass]
struct EvalHandle {
    flag: Arc<AtomicBool>,
}

#[pymethods]
impl EvalHandle {
    /// Ask this eval to stop at its next pipeline-element boundary. Safe to
    /// call before the eval has started (it will stop immediately once it
    /// acquires the engine).
    fn interrupt(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }
}

/// A persistent embedded nushell engine.
#[pyclass]
struct Engine {
    inner: Arc<Mutex<EngineInner>>,
}

#[pymethods]
impl Engine {
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                engine_state: initial_engine_state(),
                // collect_value marks the last command's stdout as
                // OutDest::Value: a trailing external (or `print`) collects
                // into the returned value instead of writing to the host
                // process stdio, which under MCP stdio transport IS the
                // protocol stream.
                stack: Stack::new().collect_value(),
            })),
        }
    }

    /// Evaluate nushell source against the persistent state.
    ///
    /// Returns `(awaitable, handle)`: the awaitable resolves to the pipeline's
    /// output as native Python objects; `handle.interrupt()` stops this eval
    /// (and only this eval) the way ctrl-c would. `input` becomes the
    /// pipeline's `$in`; `cwd`/`env` set `PWD` / environment variables for
    /// this and later calls (the stack is persistent). Raises `NuError` with
    /// nushell's rendered diagnostic.
    #[pyo3(signature = (code, input=None, cwd=None, env=None))]
    fn eval<'py>(
        &self,
        py: Python<'py>,
        code: String,
        input: Option<Bound<'py, PyAny>>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
    ) -> PyResult<(Bound<'py, PyAny>, EvalHandle)> {
        // Convert under the GIL now; the blocking task must not touch Python.
        let input = input.as_ref().map(py_to_value).transpose()?;
        let inner = Arc::clone(&self.inner);
        let flag = Arc::new(AtomicBool::new(false));
        let interrupt = Arc::clone(&flag);
        let future = pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut guard = inner
                    .lock()
                    .map_err(|_| "a previous eval panicked; create a fresh Engine".to_owned())?;
                guard.eval(&code, input, cwd, env, &interrupt)
            })
            .await
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
            match result {
                Ok(value) => Python::attach(|py| value_to_py(py, value)),
                Err(diagnostic) => Err(NuError::new_err(diagnostic)),
            }
        })?;
        Ok((future, EvalHandle { flag }))
    }
}

#[pymodule]
fn _nu(module: &Bound<'_, PyModule>) -> PyResult<()> {
    // Nushell's experimental `pipefail` option is ON by default (OptOut since
    // 0.107), and its try/catch collection path (`Instruction::TryCollect` ->
    // eval_ir.rs `collect`) waits on an external's exit status BEFORE draining
    // its stdout pipe. A child with more output pending than the OS pipe
    // buffer (64 KiB) can then never exit: it blocks in write(2), no EPIPE
    // ever arrives because this process still holds the read end, and the
    // eval deadlocks in waitpid -- wedging the engine's mutex and with it
    // every later `nu()` call in the session (indexable-inc/index#2015;
    // upstream ordering discussed in nushell/nushell#17571 / #17764, which
    // fixed the sibling `collect_reg` path but left `TryCollect` checking
    // first). Externals still fail loudly without pipefail: trailing and
    // statement externals raise through ByteStream's own consume-then-wait
    // checks, and these bindings drop the pipeline's exit-guard vector at the
    // boundary anyway, so the option bought nothing observable here.
    //
    // SAFETY: `set` is unsafe only to discourage mid-run flips; this runs
    // once at module import, before any `Engine` can exist.
    unsafe { nu_experimental::PIPE_FAIL.set(false) };
    module.add_class::<Engine>()?;
    module.add_class::<EvalHandle>()?;
    module.add("NuError", module.py().get_type::<NuError>())?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
