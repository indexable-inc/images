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

    // `CARGO_CFG_TARGET_OS` is the target, set by cargo for the build script. The
    // link search path and rpath for `-lkrun` are added by the workspace build
    // (lib/rust/workspace.nix), because a build script's `rustc-link-search` does
    // not reach the final unit link in the repo's cargo-unit graph; the `-l` does.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // macOS: libkrun-efi, which needs the embedded OVMF firmware. `-lkrun`
    // resolves to `libkrun-efi.dylib`. Forward the firmware path to a compile-time
    // env so `linuxkrun.rs` can `include_bytes!` it (self-contained across the
    // self-sign re-exec).
    if target_os == "macos"
        && let Ok(firmware) = std::env::var("KRUN_EFI_FIRMWARE")
    {
        println!("cargo:rustc-cfg=have_libkrun");
        println!("cargo:rustc-link-lib=dylib=krun");
        println!("cargo:rustc-env=KRUN_EFI_FIRMWARE={firmware}");
    }

    // Linux: classic libkrun (KVM), no firmware. `-lkrun` resolves to `libkrun.so`.
    if target_os == "linux" && std::env::var_os("VMKIT_LINK_LIBKRUN").is_some() {
        println!("cargo:rustc-cfg=have_libkrun");
        println!("cargo:rustc-link-lib=dylib=krun");
    }
}
