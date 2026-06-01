//! Best-effort Minecraft pop sounds, owned by the feed so every pusher gets the
//! right cue for free.
//!
//! Mirrors `crate::scene::Kind` to a vanilla sound: a success orb plays the
//! experience-orb pickup, a failure villager plays the villager "no" grunt. Like
//! the book overlay's page-flip, this shells out to the `minecraft-sound` binary
//! (which bundles Mojang's sound pack and returns immediately, re-spawning the
//! actual playback detached), so no audio backend is linked into the overlay and
//! playback never blocks the event loop.
//!
//! This is cosmetic: a missing binary or a failed play is logged once and then
//! ignored, so a machine without `minecraft-sound` on `PATH` (or `ORB_SOUND_CMD`)
//! simply runs silent.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;

use crate::scene::Kind;

/// The experience-orb pickup sound for a success pop.
const ORB: &str = "random/orb";

/// The villager "no / displeased" grunts for a failure pop. Cycled so repeated
/// failures do not sound identical, the way Minecraft varies its villager voice.
const VILLAGER_NO: [&str; 3] = ["mob/villager/no1", "mob/villager/no2", "mob/villager/no3"];

/// Binary that plays a named Minecraft sound. Overridable so a packaged service
/// can pin an absolute store path instead of relying on `PATH` (launchd/systemd
/// units do not inherit the interactive `PATH`).
fn sound_cmd() -> String {
    std::env::var("ORB_SOUND_CMD").unwrap_or_else(|_| "minecraft-sound".to_string())
}

/// Play the cue for `kind`. Non-blocking and best-effort.
pub fn play(kind: Kind) {
    let name = match kind {
        Kind::Orb => ORB,
        Kind::Villager => {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            VILLAGER_NO[NEXT.fetch_add(1, Ordering::Relaxed) % VILLAGER_NO.len()]
        }
    };
    match Command::new(sound_cmd())
        .args(["play", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        // Reap the launcher off the UI thread so it never zombies. The launcher
        // parses the sound index and then daemonizes the actual playback, so this
        // wait returns in tens of ms (off the UI thread either way).
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => {
            static WARN: Once = Once::new();
            WARN.call_once(|| {
                eprintln!("xp-orb-overlay: pop sound disabled ({e}); `minecraft-sound` not on PATH");
            });
        }
    }
}
