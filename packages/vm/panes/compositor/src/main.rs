//! Guest-side headless Wayland compositor: exports each xdg_toplevel over
//! vsock to `panes-host` on the macOS side. See index#1686.

fn main() {
    // Filled in by the compositor milestone (M2); the stub keeps the cargo
    // workspace resolvable while the crates land in parallel.
    eprintln!("panes-compositor: not yet implemented (index#1686)");
    std::process::exit(1);
}
