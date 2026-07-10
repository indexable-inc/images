//! Best-effort Minecraft page-flip sound.
//!
//! Turning a page in the vanilla book plays a flip sound. The overlay reproduces
//! that by shelling out to the `minecraft-sound` binary, which bundles Mojang's
//! sound pack and returns immediately (it re-spawns itself detached), so no audio
//! backend is linked into the overlay and playback never blocks the event loop.
//!
//! This is cosmetic: a missing binary or a failed play is logged once and then
//! ignored, so a machine without `minecraft-sound` on `PATH` simply runs silent.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The vanilla book page-flip sounds. This asset index ships no `page_turn`
/// event; these are the file-level names Mojang's pack exposes (and what the
/// `minecraft-sound` pack resolves).
const FLIP: [&str; 3] = [
    "item/book/open_flip1",
    "item/book/open_flip2",
    "item/book/open_flip3",
];

/// Play a page-flip sound, cycling through the three vanilla variants so repeated
/// turns do not sound identical. Non-blocking and best-effort.
pub fn page_flip() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let name = FLIP[NEXT.fetch_add(1, Ordering::Relaxed) % FLIP.len()];
    overlay_core::play_minecraft_sound(
        "BOOK_SOUND_CMD",
        name,
        "book-overlay: page-turn sound disabled",
    );
}
