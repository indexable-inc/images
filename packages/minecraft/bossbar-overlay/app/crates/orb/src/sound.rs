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

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::scene::Kind;

/// The experience-orb pickup sound for a success pop.
const ORB: &str = "random/orb";

/// The villager "no / displeased" grunts for a failure pop. Cycled so repeated
/// failures do not sound identical, the way Minecraft varies its villager voice.
const VILLAGER_NO: [&str; 3] = ["mob/villager/no1", "mob/villager/no2", "mob/villager/no3"];

/// Play the cue for `kind`. Non-blocking and best-effort.
pub fn play(kind: Kind) {
    let name = match kind {
        Kind::Orb => ORB,
        Kind::Villager => {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            VILLAGER_NO[NEXT.fetch_add(1, Ordering::Relaxed) % VILLAGER_NO.len()]
        }
    };
    overlay_core::play_minecraft_sound(
        "ORB_SOUND_CMD",
        name,
        "xp-orb-overlay: pop sound disabled",
    );
}
