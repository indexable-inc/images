//! An embedder value that arrives after the session was created still reaches
//! the next evaluation.
//!
//! `builtins.nixVersion` and `builtins.currentSystem` come from process
//! globals the embedder sets, and a session outlives any one call: the handle
//! API keeps one `Vm` across them, so `ixe_set_nix_version` can legitimately
//! land after `ixe_session_new`.
//!
//! Since ENG-12939 a `Vm` carries a settings snapshot rather than reading
//! those globals as it goes, which is what stops an evaluation running under
//! one configuration while its memo key describes another. The snapshot is
//! re-taken at the top of each evaluation entry point, and this test exists to
//! hold that: it is the only thing standing between "settings hold still
//! within a run" and "a setter after `ixe_session_new` is ignored for the life
//! of the process".
//!
//! Driven through the C ABI on purpose. Calling
//! `Vm::reload_settings_from_process` directly would prove the method works
//! and say nothing about whether the entry point calls it, which is the half
//! that can regress.
//!
//! Its own integration test binary, because these are `OnceLock`s: setting one
//! is a one-way process-wide move, so a test elsewhere would decide the
//! outcome by running first. Both transitions live here rather than in two
//! files because the two locks are independent, so each is still observed
//! exactly once.

use nix_eval_rs::capi::{
    IxeHostVtable, IxeSession, ixe_render, ixe_session_eval, ixe_session_free, ixe_session_new,
};
use nix_eval_rs::eval;

/// `IXE_OK` and `IXE_RENDER_PLAIN` from `include/nix-eval-rs.h`. Spelled as
/// literals because the header is the ABI and these are not exported to Rust.
const OK: i32 = 0;
const RENDER_PLAIN: i32 = 0;

struct Session(*mut IxeSession);

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { ixe_session_free(self.0) };
    }
}

impl Session {
    fn new() -> Self {
        // Nothing here asks the outside world anything, so the host answers
        // nothing; the subject is the process globals a session snapshots.
        let vtable = IxeHostVtable::empty();
        // SAFETY: points at a live local for the duration of the call, which
        // is all `ixe_session_new` needs -- it copies the struct.
        Session(unsafe { ixe_session_new(&raw const vtable) })
    }

    /// Evaluate `source` and render the result, the way the bridge does.
    fn eval(&self, source: &str) -> Result<String, i32> {
        let mut handle = 0u64;
        let status = unsafe {
            ixe_session_eval(
                self.0,
                source.as_ptr(),
                source.len(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                &raw mut handle,
            )
        };
        if status != OK {
            return Err(status);
        }
        let mut out: *mut std::ffi::c_char = std::ptr::null_mut();
        let status = unsafe { ixe_render(self.0, handle, RENDER_PLAIN, &raw mut out) };
        let text = if out.is_null() {
            String::new()
        } else {
            let owned = unsafe { std::ffi::CStr::from_ptr(out) }
                .to_string_lossy()
                .into_owned();
            unsafe { nix_eval_rs::capi::ixe_string_free(out) };
            owned
        };
        if status == OK { Ok(text) } else { Err(status) }
    }
}

#[test]
fn a_session_created_before_the_version_was_set_still_reports_it() {
    let session = Session::new();
    // Before: no embedder has said, so the slot is the unimplemented one and
    // this is an error rather than a value. Asserted so the pass below is a
    // transition and not a state the process was already in.
    assert!(
        session.eval("builtins.nixVersion").is_err(),
        "a version was already set; this binary must be the only one setting it"
    );

    assert!(eval::set_nix_version("9.9.9-eng-12539").is_ok());

    assert_eq!(
        session.eval("builtins.nixVersion").as_deref(),
        Ok("\"9.9.9-eng-12539\""),
        "the session evaluated against the settings it was built with, so a \
         setter called after `ixe_session_new` is invisible for the life of \
         the process"
    );
}

#[test]
fn a_session_created_before_the_system_was_set_still_reports_it() {
    let session = Session::new();
    assert!(
        session.eval("builtins.currentSystem").is_err(),
        "a system was already set; this binary must be the only one setting it"
    );

    assert!(eval::set_current_system("eng12539-linux").is_ok());

    assert_eq!(
        session.eval("builtins.currentSystem").as_deref(),
        Ok("\"eng12539-linux\""),
        "the session evaluated against the settings it was built with, so a \
         setter called after `ixe_session_new` is invisible for the life of \
         the process"
    );
}
