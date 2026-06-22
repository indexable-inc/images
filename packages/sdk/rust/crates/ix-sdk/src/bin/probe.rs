//! Runtime probe: links the prebuilt `ix-sdk-wire` rlib through the public SDK
//! and prints a value computed inside it. The nix check runs this binary and
//! asserts the output, proving the consumer linked and ran the prebuilt rlib
//! (not just typechecked against the rmeta).

fn main() {
    // `Unknown = 0` round-trips to 0 (the reserved sentinel). The call resolves
    // only if the prebuilt rlib was linked: the stub source defines no such fn.
    let code = ix_sdk::normalize_error_code(0);
    // `from_u32` of an out-of-range value also folds to `Unknown` (0), per the
    // append-only / forward-compat contract in the real crate.
    let unknown = ix_sdk::normalize_error_code(u32::MAX);
    println!("ix-sdk-wire linked: normalize(0)={code} normalize(MAX)={unknown}");
}
