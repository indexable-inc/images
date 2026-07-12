//! Best-effort, non-blocking playback through the `minecraft-sound` launcher.

use std::process::{Command, Stdio};
use std::sync::Once;

/// Spawn a named sound and reap its short-lived launcher off the UI thread.
/// A missing launcher is cosmetic, so it is reported only once per process.
pub fn play_minecraft_sound(env_var: &str, name: &str, context: &str) {
    let command = std::env::var(env_var).unwrap_or_else(|_| "minecraft-sound".to_owned());
    match Command::new(command)
        .args(["play", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => {
            static WARN: Once = Once::new();
            WARN.call_once(|| {
                eprintln!("{context} ({error}); `minecraft-sound` not on PATH");
            });
        }
    }
}
