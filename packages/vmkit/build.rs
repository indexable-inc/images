//! Build script for `vmkit`.
//!
//! Links libkrun for the Linux-guest backend (`src/linuxkrun.rs`) when it is
//! available for the build host:
//! - **aarch64-darwin**: the `libkrun-efi` dylib plus the embedded OVMF firmware.
//!   The workspace build sets `KRUN_EFI_FIRMWARE` (only on a native aarch64-darwin
//!   build, where nixpkgs provides libkrun-efi); its presence is the signal.
//! - **Linux**: classic KVM `libkrun` (no firmware). The workspace build sets
//!   `VMKIT_LINK_LIBKRUN` when nixpkgs `libkrun` is available for the host.
//!
//! Everywhere else (x86_64-darwin, a Linux->darwin cross build) neither env is
//! set, so the `have_libkrun` cfg stays unset and the crate compiles the typed
//! stub backend.

fn main() {
    // Declare the custom cfg unconditionally so the `unexpected_cfgs` lint accepts
    // `#[cfg(have_libkrun)]` on every platform.
    println!("cargo:rustc-check-cfg=cfg(have_libkrun)");
    println!("cargo:rerun-if-env-changed=KRUN_EFI_FIRMWARE");
    println!("cargo:rerun-if-env-changed=VMKIT_LINK_LIBKRUN");

    // `CARGO_CFG_TARGET_OS` is the target, set by cargo for the build script.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "macos" => {
            // libkrun-efi: needs the embedded OVMF firmware. The link search path
            // and rpath are added by the workspace build (lib/rust/workspace.nix),
            // because a build script's `rustc-link-search` does not reach the final
            // unit link in the repo's cargo-unit graph; the `-l` directive does.
            if let Ok(firmware) = std::env::var("KRUN_EFI_FIRMWARE") {
                println!("cargo:rustc-cfg=have_libkrun");
                // `-lkrun` resolves to `libkrun-efi.dylib` via the nix package.
                println!("cargo:rustc-link-lib=dylib=krun");
                // Forward the firmware path to a compile-time env so `linuxkrun.rs`
                // can `include_bytes!` it (self-contained across the self-sign
                // re-exec).
                println!("cargo:rustc-env=KRUN_EFI_FIRMWARE={firmware}");
            }
        }
        "linux" => {
            // classic libkrun (KVM): no firmware. `-lkrun` resolves to `libkrun.so`;
            // the search path/rpath come from the workspace build.
            if std::env::var_os("VMKIT_LINK_LIBKRUN").is_some() {
                println!("cargo:rustc-cfg=have_libkrun");
                println!("cargo:rustc-link-lib=dylib=krun");
            }
        }
        _ => {}
    }
}
