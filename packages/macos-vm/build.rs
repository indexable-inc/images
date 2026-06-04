//! Build script for `macos-vm`.
//!
//! On macOS the crate links the libkrun-efi dylib (for the Linux-guest path,
//! `src/linuxkrun.rs`) and embeds the OVMF firmware blob. On Linux the crate is
//! a typed "macOS only" stub, so this is a no-op there.

fn main() {
    // `CARGO_CFG_TARGET_OS` is the target, set by cargo for the build script.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    // Link libkrun-efi. `-lkrun` resolves to `libkrun-efi.dylib` via the symlink
    // chain the nix package provides. The link *search path* and rpath are added
    // by the workspace build (lib/rust/workspace.nix), because a build script's
    // `rustc-link-search` does not reach the final unit link in the repo's
    // cargo-unit graph (the same reason the ALSA lib dir is added there); the
    // `-l` directive below does propagate.
    println!("cargo:rustc-link-lib=dylib=krun");

    // Forward the OVMF firmware path to a compile-time env so `linuxkrun.rs` can
    // `include_bytes!` it into the binary (self-contained across the entitlement
    // self-sign re-exec). The nix build sets `KRUN_EFI_FIRMWARE` to the libkrun
    // source's `edk2/KRUN_EFI.silent.fd`.
    println!("cargo:rerun-if-env-changed=KRUN_EFI_FIRMWARE");
    if let Ok(firmware) = std::env::var("KRUN_EFI_FIRMWARE") {
        println!("cargo:rustc-env=KRUN_EFI_FIRMWARE={firmware}");
    }
}
