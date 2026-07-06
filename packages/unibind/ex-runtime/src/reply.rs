//! Run a future on the runtime and message the calling process.

use std::future::Future;
use std::sync::Mutex;

use rustler::env::OwnedEnv;
use rustler::{Encoder, Env, LocalPid, Monitor, NifResult, Resource, ResourceArc, Term};
use tokio::task::AbortHandle;

use crate::atoms;
use crate::runtime::runtime;

/// A handle to one in-flight async call.
///
/// The generated NIF returns it so the caller keeps the task alive; the
/// process monitor aborts the task when the caller exits first.
pub struct InFlight {
    abort: Mutex<Option<AbortHandle>>,
}

#[rustler::resource_impl]
impl Resource for InFlight {
    fn down<'a>(&'a self, _env: Env<'a>, _pid: LocalPid, _monitor: Monitor) {
        abort_stored(&self.abort);
    }
}

/// Abort the task stored in `slot`, if it is still there.
pub fn abort_stored(slot: &Mutex<Option<AbortHandle>>) {
    if let Ok(mut guard) = slot.lock()
        && let Some(handle) = guard.take()
    {
        handle.abort();
    }
}

/// An uninhabited error type: `Result<T, Never>` gives async functions
/// without a `throws` the same `{:ok, value}` wire shape as throwing ones.
pub enum Never {}

impl Encoder for Never {
    // `Encoder::encode` takes `&self`, and no `&Never` can exist.
    #[expect(clippy::uninhabited_references, reason = "the trait dictates &self")]
    fn encode<'a>(&self, _env: Env<'a>) -> Term<'a> {
        match *self {}
    }
}

/// Run `fut` on the shared runtime and send its output to the calling
/// process as `{:unibind, reference, {:ok, value} | {:error, error}}`.
///
/// The task is aborted when the caller exits before the reply, observable
/// to the user only as their `Drop` impls running.
///
/// # Errors
///
/// Never fails today; the `NifResult` keeps the signature open for
/// registration-time failures.
///
/// # Panics
///
/// Panics when `env` is not a process-bound NIF environment.
pub fn spawn_reply<F, T, E>(
    env: Env<'_>,
    reference: Term<'_>,
    fut: F,
) -> NifResult<ResourceArc<InFlight>>
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: Encoder + Send + 'static,
    E: Encoder + Send + 'static,
{
    let pid = env.pid();
    let mut owned = OwnedEnv::new();
    let saved = owned.run(|inner| owned.save(reference.in_env(inner)));
    let resource = ResourceArc::new(InFlight {
        abort: Mutex::new(None),
    });
    let task = runtime().spawn(async move {
        let out = fut.await;
        let _ = owned.send_and_clear(&pid, |env| {
            let reference = saved.load(env);
            (atoms::unibind(), reference, out).encode(env)
        });
    });
    store_and_monitor(env, &resource, &resource.abort, task.abort_handle(), pid);
    Ok(resource)
}

/// Store the task's abort handle in its resource, then monitor the caller;
/// storing first means a `down` can never miss the handle. A dead caller
/// (monitor refused) aborts immediately.
pub fn store_and_monitor<T: Resource>(
    env: Env<'_>,
    resource: &ResourceArc<T>,
    slot: &Mutex<Option<AbortHandle>>,
    handle: AbortHandle,
    pid: LocalPid,
) {
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(handle);
    }
    if env.monitor(resource, &pid).is_none() {
        abort_stored(slot);
    }
}
