//! CLI surface, in vmkit's style: long flags, doc comments as help text.

use std::path::PathBuf;

#[derive(Debug, clap::Parser)]
#[command(
    name = "panes-compositor",
    about = "Headless Wayland compositor exporting each xdg_toplevel to a macOS host over a byte stream"
)]
pub struct Cli {
    /// `AF_VSOCK` port to listen on: the production transport inside the VM
    /// (panes-host reaches it through libkrun's vsock port map). Ignored when
    /// --listen-unix or --listen-tcp is given.
    #[arg(long, value_name = "PORT", default_value_t = panes_protocol::VSOCK_PORT)]
    pub listen_vsock: u32,

    /// Listen on a unix socket instead of vsock (off-VM development; a stale
    /// socket file at PATH is removed).
    #[arg(long, value_name = "PATH", conflicts_with = "listen_tcp")]
    pub listen_unix: Option<PathBuf>,

    /// Listen on TCP instead of vsock (off-VM development), e.g.
    /// "127.0.0.1:7100".
    #[arg(long, value_name = "ADDR")]
    pub listen_tcp: Option<String>,

    /// XKB keymap layout for the virtual keyboard ("us", "de", ...). The host
    /// sends evdev keycodes only; clients receive this keymap and interpret.
    #[arg(long, default_value = "us")]
    pub xkb_layout: String,

    /// Wayland socket name clients connect to (their `WAYLAND_DISPLAY`),
    /// created under `XDG_RUNTIME_DIR`.
    #[arg(long, default_value = "wayland-1")]
    pub socket_name: String,

    /// Log filter: a level ("info") or tracing directives
    /// ("info,smithay=warn").
    #[arg(long, default_value = "info")]
    pub log_level: String,
}
