//! User-supplied themed boss bar texture sets.
//!
//! A bar row with a non-empty `theme` column renders from a texture set on
//! disk instead of the vanilla color sprites. Each theme is one directory
//! under the themes root:
//!
//! ```text
//! <themes root>/<name>/
//!   background.png            required: the empty track (vanilla art is 182x5)
//!   progress.png              required: the filled track, cropped to the fill
//!   notched_6_background.png  optional: per-notch overlay overrides; when a
//!   notched_6_progress.png    pair is absent the vanilla notch sprites layer
//!   ...                       on top of the themed track as usual
//! ```
//!
//! The themes root is `$BOSSBAR_THEMES` when set, else `themes/` next to the
//! database (so `~/.local/share/bossbar-overlay/themes/wither/...` by
//! default). Sprites slot into the existing four-layer renderer untouched:
//! background, progress (cropped to the fill), notch background, notch
//! progress. Any PNG size works; it is drawn into the bar's 182:5 box.
//!
//! LICENSING: nothing here ships any third-party art, fetches it, or knows
//! pack names. The user supplies a directory of textures they are licensed to
//! use (e.g. sliced out of a resource pack they bought or made; see
//! `scripts/import-theme.sh`). A missing or broken theme falls back to the
//! vanilla `color` sprites, the same "a typo still draws a bar" policy the
//! rest of the row contract follows.

use std::collections::HashMap;
use std::path::PathBuf;

use overlay_core::{Gpu, TexHandle};

use crate::bars::Notch;
use crate::db;

/// Uploaded textures for one theme. `notch` only holds the variants the theme
/// directory actually provides; the renderer falls back to the vanilla notch
/// sprites for the rest.
#[derive(Clone)]
pub struct ThemeSprites {
    pub bg: TexHandle,
    pub fill: TexHandle,
    notch: HashMap<&'static str, (TexHandle, TexHandle)>,
}

impl ThemeSprites {
    /// The theme's own notch overlay pair, when it ships one.
    pub fn notch(&self, n: Notch) -> Option<(TexHandle, TexHandle)> {
        self.notch.get(notch_stem(n)).copied()
    }
}

fn notch_stem(n: Notch) -> &'static str {
    match n {
        Notch::N6 => "notched_6",
        Notch::N10 => "notched_10",
        Notch::N12 => "notched_12",
        Notch::N20 => "notched_20",
    }
}

const NOTCH_STEMS: [&str; 4] = ["notched_6", "notched_10", "notched_12", "notched_20"];

/// The themes root: `$BOSSBAR_THEMES`, else `themes/` next to the database.
/// Derived from the DB path so `BOSSBAR_DB=/tmp/x/bars.db` keeps everything
/// (db + themes) under one caller-chosen root.
pub fn themes_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BOSSBAR_THEMES") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    let db = db::resolve_path();
    db.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("themes")
}

/// A theme name is a single path component: letters, digits, `.`/`_`/`-`, not
/// starting with a dot. The name comes off an untrusted DB row and is joined
/// onto the themes root, so this is the traversal guard (`../../etc` is just
/// an unknown theme, which draws vanilla).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Theme name -> uploaded sprites, memoized like the icon cache so a theme's
/// PNGs upload once, not per reconcile. `None` records a theme whose required
/// sprites failed to decode (retrying cannot help); a theme directory that
/// does not exist *yet* is not cached, so dropping the files in while the
/// overlay runs is picked up on the next re-resolve.
pub struct ThemeCache {
    dir: PathBuf,
    loaded: HashMap<String, Option<ThemeSprites>>,
}

impl ThemeCache {
    pub fn new() -> Self {
        Self {
            dir: themes_dir(),
            loaded: HashMap::new(),
        }
    }

    /// Resolve a bar's `theme` value. Empty, invalid, missing, or broken all
    /// yield `None`, which the scene builder renders as the vanilla sprites.
    pub fn resolve(&mut self, gpu: &mut Gpu, name: &str) -> Option<ThemeSprites> {
        if !valid_name(name) {
            return None;
        }
        if let Some(cached) = self.loaded.get(name) {
            return cached.clone();
        }
        let root = self.dir.join(name);
        if !root.is_dir() {
            // Transient: the user may not have imported the set yet.
            return None;
        }
        let loaded = load(gpu, &root);
        if loaded.is_none() {
            eprintln!(
                "bossbar-overlay: theme '{name}' at {} is missing background.png/progress.png (or they failed to decode); using vanilla sprites",
                root.display()
            );
        }
        self.loaded.insert(name.to_string(), loaded.clone());
        loaded
    }
}

/// Load one theme directory. Both base sprites are required; notch overrides
/// are per-pair optional.
fn load(gpu: &mut Gpu, root: &std::path::Path) -> Option<ThemeSprites> {
    let mut register = |stem: &str| -> Option<TexHandle> {
        let bytes = std::fs::read(root.join(format!("{stem}.png"))).ok()?;
        gpu.register_image_scaled(&bytes, MAX_SPRITE_PX)
    };
    let bg = register("background")?;
    let fill = register("progress")?;
    let mut notch = HashMap::new();
    for stem in NOTCH_STEMS {
        if let (Some(nbg), Some(nfill)) = (
            register(&format!("{stem}_background")),
            register(&format!("{stem}_progress")),
        ) {
            notch.insert(stem, (nbg, nfill));
        }
    }
    Some(ThemeSprites { bg, fill, notch })
}

/// Largest dimension a theme sprite is downscaled to on upload. Generous (a
/// 182-wide bar drawn at scale 4 on a 2x display is still under 1500px) while
/// bounding the GPU memory an arbitrary on-disk PNG can claim.
const MAX_SPRITE_PX: u32 = 2048;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_single_path_components() {
        assert!(valid_name("wither"));
        assert!(valid_name("ender-dragon_v2.1"));
        assert!(!valid_name(""));
        assert!(!valid_name("../escape"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("nul\0byte"));
    }
}
