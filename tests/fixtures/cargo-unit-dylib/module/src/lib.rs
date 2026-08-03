//! The `dlopen`ed side. Reaches the engine's counter through the shared image.

/// # Safety
/// Called through `dlsym` by the host binary; takes no arguments and touches
/// only the engine's atomics.
#[unsafe(no_mangle)]
pub extern "C" fn cargo_unit_module_bump() -> u64 {
    cargo_unit_dylib_engine::bump()
}
