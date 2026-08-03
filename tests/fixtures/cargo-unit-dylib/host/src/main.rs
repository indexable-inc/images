//! Bumps the engine's counter on both sides of a `dlopen` boundary and checks
//! they are the same counter.

use std::{env, ffi::CString};

unsafe extern "C" {
    fn dlopen(filename: *const core::ffi::c_char, flag: core::ffi::c_int) -> *mut core::ffi::c_void;
    fn dlsym(
        handle: *mut core::ffi::c_void,
        symbol: *const core::ffi::c_char,
    ) -> *mut core::ffi::c_void;
    fn dlerror() -> *const core::ffi::c_char;
}

// Same value on Linux and macOS.
const RTLD_NOW: core::ffi::c_int = 2;

fn main() {
    assert_eq!(
        cargo_unit_dylib_engine::probe(),
        42,
        "the native archive symbol re-exported by build.rs did not answer"
    );

    let path = env::args()
        .nth(1)
        .expect("usage: cargo-unit-dylib-host <module dylib>");
    let path = CString::new(path).expect("module path contained a NUL");

    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
    assert!(!handle.is_null(), "dlopen failed: {}", last_dl_error());

    let symbol = CString::new("cargo_unit_module_bump").expect("static name");
    let address = unsafe { dlsym(handle, symbol.as_ptr()) };
    assert!(!address.is_null(), "dlsym failed: {}", last_dl_error());
    let module_bump: extern "C" fn() -> u64 = unsafe { std::mem::transmute(address) };

    // One counter shared through one engine image gives 0, 1, 2. Two static
    // copies give 0, 0, 1 -- the host and the module each counting their own.
    let host_first = cargo_unit_dylib_engine::bump();
    let from_module = module_bump();
    let host_total = cargo_unit_dylib_engine::count();

    assert_eq!(host_first, 0, "the host's first bump should see a fresh counter");
    assert_eq!(
        from_module, 1,
        "the module saw its own engine copy: {from_module} instead of 1"
    );
    assert_eq!(
        host_total, 2,
        "the host and the module do not share one engine: {host_total} instead of 2"
    );

    println!("one engine: host={host_first} module={from_module} total={host_total}");
}

fn last_dl_error() -> String {
    let error = unsafe { dlerror() };
    if error.is_null() {
        return "(no dlerror)".to_string();
    }
    unsafe { std::ffi::CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}
