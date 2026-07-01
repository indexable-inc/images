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
    for (key, value) in std::env::vars() {
        engine_state.add_env_var(key, Value::string(value, Span::unknown()));
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
    ) -> Result<Value, String> {
        let Self {
            engine_state,
            stack,
        } = self;

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
                if list.len() > MAX_RANGE_ELEMENTS {
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
    if let Ok(val) = object.extract::<i64>() {
        return Ok(Value::int(val, span));
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
    if let Ok(val) = object.extract::<Vec<u8>>() {
        return Ok(Value::binary(val, span));
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

/// A persistent embedded nushell engine.
#[pyclass]
struct Engine {
    inner: Arc<Mutex<EngineInner>>,
    interrupt: Arc<AtomicBool>,
}

#[pymethods]
impl Engine {
    #[new]
    fn new() -> Self {
        let mut engine_state = initial_engine_state();
        let interrupt = Arc::new(AtomicBool::new(false));
        engine_state.set_signals(Signals::new(interrupt.clone()));
        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                engine_state,
                stack: Stack::new(),
            })),
            interrupt,
        }
    }

    /// Evaluate nushell source against the persistent state.
    ///
    /// Returns an awaitable resolving to the pipeline's output as native
    /// Python objects. `input` becomes the pipeline's `$in`; `cwd`/`env` set
    /// `PWD` / environment variables for this and later calls (the stack is
    /// persistent). Raises `NuError` with nushell's rendered diagnostic.
    #[pyo3(signature = (code, input=None, cwd=None, env=None))]
    fn eval<'py>(
        &self,
        py: Python<'py>,
        code: String,
        input: Option<Bound<'py, PyAny>>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // Convert under the GIL now; the blocking task must not touch Python.
        let input = input.as_ref().map(py_to_value).transpose()?;
        let inner = Arc::clone(&self.inner);
        let interrupt = Arc::clone(&self.interrupt);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut guard = inner
                    .lock()
                    .map_err(|_| "a previous eval panicked; create a fresh Engine".to_owned())?;
                // Reset the interrupt only AFTER winning the lock: resetting
                // before it could erase an interrupt aimed at the eval still
                // holding the mutex (which would then never stop, wedging
                // both). Inside the guard, the reset provably applies to this
                // eval alone.
                interrupt.store(false, Ordering::Relaxed);
                guard.eval(&code, input, cwd, env)
            })
            .await
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
            match result {
                Ok(value) => Python::attach(|py| value_to_py(py, value)),
                Err(diagnostic) => Err(NuError::new_err(diagnostic)),
            }
        })
    }

    /// Ask the running evaluation to stop at the next pipeline-element
    /// boundary (the same mechanism as ctrl-c in the nushell CLI).
    fn interrupt(&self) {
        self.interrupt.store(true, Ordering::Relaxed);
    }
}

#[pymodule]
fn _nu(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Engine>()?;
    module.add("NuError", module.py().get_type::<NuError>())?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
