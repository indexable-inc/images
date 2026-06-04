//! Build script for `macos-vm`.
//!
//! When building natively on aarch64-darwin the crate links the libkrun-efi
//! dylib (for the Linux-guest path, `src/linuxkrun.rs`) and embeds the OVMF
//! firmware blob. Everywhere else (Linux, x86_64-darwin, and Linux->darwin cross
//! builds) libkrun-efi is unavailable, so the `have_libkrun` cfg is left unset
//! and the crate compiles without the libkrun backend.

fn main() {
    // Declare the custom cfg unconditionally so the `unexpected_cfgs` lint
    // accepts `#[cfg(have_libkrun)]` on every platform.
    println!("cargo:rustc-check-cfg=cfg(have_libkrun)");
    println!("cargo:rerun-if-env-changed=KRUN_EFI_FIRMWARE");

    // `CARGO_CFG_TARGET_OS` is the target, set by cargo for the build script.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    // The workspace build sets `KRUN_EFI_FIRMWARE` (the embedded OVMF blob) only
    // when the build host is aarch64-darwin, where nixpkgs provides libkrun-efi
    // (it is not cross-buildable from Linux). Use its presence as the signal that
    // the libkrun backend can be linked: a Linux->darwin cross build has neither
    // the firmware nor the dylib, so it compiles the crate without libkrun.
    if let Ok(firmware) = std::env::var("KRUN_EFI_FIRMWARE") {
        // Enable the libkrun backend in `linuxkrun.rs`.
        println!("cargo:rustc-cfg=have_libkrun");
        // Link libkrun-efi. `-lkrun` resolves to `libkrun-efi.dylib` via the
        // symlink chain the nix package provides. The link *search path* and
        // rpath are added by the workspace build (lib/rust/workspace.nix),
        // because a build script's `rustc-link-search` does not reach the final
        // unit link in the repo's cargo-unit graph; the `-l` directive does.
        println!("cargo:rustc-link-lib=dylib=krun");
        // Forward the firmware path to a compile-time env so `linuxkrun.rs` can
        // `include_bytes!` it into the binary (self-contained across the
        // entitlement self-sign re-exec).
        println!("cargo:rustc-env=KRUN_EFI_FIRMWARE={firmware}");
    }
}
