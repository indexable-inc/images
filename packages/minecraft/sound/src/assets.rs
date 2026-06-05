use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use snafu::{OptionExt as _, ResultExt as _, Snafu};

/// Errors that can occur while locating or resolving Minecraft sound assets.
#[derive(Debug, Snafu)]
pub enum AssetError {
    #[snafu(display("Could not determine home directory"))]
    NoHomeDir,

    #[snafu(display(
        "Minecraft directory not found at {}. Set MCSOUND_ASSETS or MINECRAFT_HOME, or install Minecraft.",
        path.display()
    ))]
    MinecraftDirMissing { path: PathBuf },

    #[snafu(display("No asset indexes found. Have you launched Minecraft at least once?"))]
    NoAssetIndexes,

    #[snafu(display("No asset index files found in {}", dir.display()))]
    NoIndexFiles { dir: PathBuf },

    #[snafu(display("Failed to read directory {}", path.display()))]
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to read directory entry in {}", path.display()))]
    ReadDirEntry {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to read file type for {}", path.display()))]
    ReadFileType {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to read index file: {}", path.display()))]
    ReadIndex {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to parse asset index JSON: {}", path.display()))]
    ParseIndex {
        path: PathBuf,
        source: serde_json::Error,
    },

    /// The requested sound name does not match any bundled or installed sound.
    /// The display lists the closest matches and points at `list` so a typo'd
    /// name fails loudly instead of silently playing nothing.
    #[snafu(display("{}", format_unknown_sound(name, suggestions)))]
    UnknownSound {
        name: String,
        suggestions: Vec<String>,
    },

    #[snafu(display("Malformed hash for {name}: {hash}"))]
    MalformedHash { name: String, hash: String },

    #[snafu(display("Sound file missing from disk: {}", path.display()))]
    SoundFileMissing { path: PathBuf },
}

#[derive(Debug, Deserialize)]
struct AssetIndex {
    objects: HashMap<String, AssetObject>,
}

#[derive(Debug, Deserialize)]
struct AssetObject {
    hash: String,
}

/// Where sound `.ogg` files are read from.
pub enum MinecraftAssets {
    /// A flat `<name>.ogg` tree produced by the Nix sound pack and pointed at
    /// by `MCSOUND_ASSETS`. This needs no Minecraft install on the machine.
    Bundled { root: PathBuf },
    /// A real Minecraft install: the asset index maps sound names to hashed
    /// objects under `assets/objects/`.
    Install {
        minecraft_dir: PathBuf,
        objects: HashMap<String, String>,
    },
}

impl MinecraftAssets {
    /// Resolve the sound backend. A `MCSOUND_ASSETS` directory wins so the
    /// Nix-packaged build works with zero configuration; otherwise fall back
    /// to auto-detecting a Minecraft (or `MINECRAFT_HOME`) install.
    ///
    /// # Errors
    /// Returns an error if no install can be found or its asset index cannot
    /// be read or parsed.
    pub fn load() -> Result<Self, AssetError> {
        if let Some(dir) = env::var_os("MCSOUND_ASSETS") {
            let root = PathBuf::from(dir);
            if root.is_dir() {
                return Ok(Self::Bundled { root });
            }
        }

        let minecraft_dir = find_minecraft_dir()?;
        let index_path = find_latest_index(&minecraft_dir)?;
        let content =
            fs::read_to_string(&index_path).context(ReadIndexSnafu { path: &index_path })?;
        let index: AssetIndex =
            serde_json::from_str(&content).context(ParseIndexSnafu { path: &index_path })?;

        let objects = index
            .objects
            .into_iter()
            .filter(|(key, _)| key.starts_with("minecraft/sounds/") && is_ogg(key))
            .map(|(key, value)| (key, value.hash))
            .collect();

        Ok(Self::Install {
            minecraft_dir,
            objects,
        })
    }

    /// List available sound names, optionally keeping only those containing
    /// `pattern`. Names are the path with the `minecraft/sounds/` prefix and
    /// `.ogg` suffix stripped, e.g. `mob/zombie/death`.
    ///
    /// # Errors
    /// Returns an error if a bundled sound directory cannot be walked.
    pub fn list_sounds(&self, pattern: Option<&str>) -> Result<Vec<String>, AssetError> {
        let mut sounds = self.all_sounds()?;
        if let Some(pattern) = pattern {
            sounds.retain(|name| name.contains(pattern));
        }
        sounds.sort();
        Ok(sounds)
    }

    /// Every known sound name, unsorted and unfiltered.
    fn all_sounds(&self) -> Result<Vec<String>, AssetError> {
        match self {
            Self::Bundled { root } => {
                let mut names = Vec::new();
                collect_bundled(root, root, &mut names)?;
                Ok(names)
            }
            Self::Install { objects, .. } => Ok(objects
                .keys()
                .filter_map(|key| {
                    key.strip_prefix("minecraft/sounds/")?
                        .strip_suffix(".ogg")
                        .map(str::to_owned)
                })
                .collect()),
        }
    }

    /// Resolve a sound name (e.g. `mob/zombie/death`) to a file on disk.
    ///
    /// # Errors
    /// Returns [`AssetError::UnknownSound`] (with close-match suggestions) when
    /// the name matches no bundled or installed sound, and other variants when
    /// the resolved file is missing or the install metadata is malformed.
    pub fn resolve_sound(&self, name: &str) -> Result<PathBuf, AssetError> {
        let path = match self {
            Self::Bundled { root } => {
                let path = root.join(format!("{name}.ogg"));
                // A bundled tree has no name index, so a missing file is the
                // only signal that the name is bogus. Treat it as "unknown
                // sound" with suggestions rather than a bare missing-file error.
                if !path.exists() {
                    return Err(self.unknown_sound(name)?);
                }
                path
            }
            Self::Install {
                minecraft_dir,
                objects,
            } => {
                let key = format!("minecraft/sounds/{name}.ogg");
                let Some(hash) = objects.get(&key) else {
                    return Err(self.unknown_sound(name)?);
                };
                let prefix = hash.get(..2).context(MalformedHashSnafu { name, hash })?;
                minecraft_dir
                    .join("assets")
                    .join("objects")
                    .join(prefix)
                    .join(hash)
            }
        };

        if path.exists() {
            Ok(path)
        } else {
            SoundFileMissingSnafu { path }.fail()
        }
    }

    /// Build an [`AssetError::UnknownSound`] for `name`, attaching the closest
    /// known sound names so the caller can suggest a correction. The inner
    /// `Result` is the failure to enumerate known sounds (e.g. unreadable dir).
    fn unknown_sound(&self, name: &str) -> Result<AssetError, AssetError> {
        let all = self.all_sounds()?;
        let suggestions = closest_matches(name, &all);
        Ok(UnknownSoundSnafu {
            name: name.to_owned(),
            suggestions,
        }
        .build())
    }
}

/// Render the user-facing message for an unknown sound: name the bad input,
/// list any close matches, and always point at `minecraft-sound list`.
fn format_unknown_sound(name: &str, suggestions: &[String]) -> String {
    let mut msg = format!("Unknown sound: '{name}'.");
    if !suggestions.is_empty() {
        msg.push_str(" Did you mean: ");
        msg.push_str(&suggestions.join(", "));
        msg.push('?');
    }
    msg.push_str(" Run `minecraft-sound list` to see available sounds.");
    msg
}

/// Pick up to three of the closest sound names to `query`, preferring substring
/// matches and then small edit distances. Keeps the dependency surface at zero
/// by using a plain Levenshtein implementation.
fn closest_matches(query: &str, candidates: &[String]) -> Vec<String> {
    let mut scored: Vec<(usize, &String)> = candidates
        .iter()
        .map(|candidate| {
            // Substring hits sort first by collapsing their distance to 0.
            let distance = if candidate.contains(query) || query.contains(candidate.as_str()) {
                0
            } else {
                levenshtein(query, candidate)
            };
            (distance, candidate)
        })
        .collect();

    // Sort by distance, then name for a stable, deterministic order.
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    // Only keep suggestions that are reasonably close: a distance larger than
    // the query length is almost certainly noise.
    let threshold = query.chars().count().max(3);
    scored
        .into_iter()
        .filter(|(distance, _)| *distance <= threshold)
        .take(3)
        .map(|(_, name)| name.clone())
        .collect()
}

/// Classic dynamic-programming Levenshtein edit distance over Unicode scalar
/// values. Small inputs (sound names), so the quadratic table is fine.
fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, a_char) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, &b_char) in b_chars.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

/// Recursively collect `<name>` for every `<name>.ogg` under `root`.
fn collect_bundled(root: &Path, dir: &Path, names: &mut Vec<String>) -> Result<(), AssetError> {
    for entry in fs::read_dir(dir).context(ReadDirSnafu { path: dir })? {
        let entry = entry.context(ReadDirEntrySnafu { path: dir })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .context(ReadFileTypeSnafu { path: &path })?;

        if file_type.is_dir() {
            collect_bundled(root, &path, names)?;
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ogg"))
            && let Some(name) = path
                .strip_prefix(root)
                .ok()
                .and_then(Path::to_str)
                .and_then(|rel| rel.strip_suffix(".ogg"))
        {
            names.push(name.to_owned());
        }
    }
    Ok(())
}

/// Case-insensitive `.ogg` extension check for asset-index keys (plain `&str`).
fn is_ogg(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ogg"))
}

/// Auto-detect the Minecraft directory, honoring `MINECRAFT_HOME` first.
fn find_minecraft_dir() -> Result<PathBuf, AssetError> {
    if let Some(home) = env::var_os("MINECRAFT_HOME") {
        let path = PathBuf::from(home);
        if path.exists() {
            return Ok(path);
        }
    }

    let path = if cfg!(target_os = "macos") {
        dirs::home_dir().map(|home| home.join("Library/Application Support/minecraft"))
    } else if cfg!(target_os = "windows") {
        dirs::data_dir().map(|data| data.join(".minecraft"))
    } else {
        dirs::home_dir().map(|home| home.join(".minecraft"))
    };

    let path = path.context(NoHomeDirSnafu)?;
    if path.exists() {
        Ok(path)
    } else {
        MinecraftDirMissingSnafu { path }.fail()
    }
}

/// Numeric version of an asset-index file (`30.json` -> `Some(30)`).
fn index_version(entry: &fs::DirEntry) -> Option<u32> {
    entry
        .path()
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.parse().ok())
}

/// Pick the highest-versioned `assets/indexes/<n>.json`.
fn find_latest_index(minecraft_dir: &Path) -> Result<PathBuf, AssetError> {
    let indexes_dir = minecraft_dir.join("assets").join("indexes");
    if !indexes_dir.exists() {
        return NoAssetIndexesSnafu.fail();
    }

    let indexes: Vec<fs::DirEntry> = fs::read_dir(&indexes_dir)
        .context(ReadDirSnafu { path: &indexes_dir })?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .collect();

    indexes
        .into_iter()
        .max_by(|a, b| {
            index_version(a)
                .cmp(&index_version(b))
                .then_with(|| a.file_name().cmp(&b.file_name()))
        })
        .map(|entry| entry.path())
        .context(NoIndexFilesSnafu { dir: indexes_dir })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// Monotonic counter so each `bundled_with` call gets its own temp dir even
    /// when libtest runs cases in parallel; identical literal slices would
    /// otherwise share an address and collide under `{:p}`.
    static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    /// A `Bundled` backend over a temp dir, paired with that dir's path for cleanup.
    struct BundledFixture {
        assets: MinecraftAssets,
        root: PathBuf,
    }

    /// Build a `Bundled` backend over a fresh temp dir holding `.ogg` files.
    fn bundled_with(names: &[&str]) -> BundledFixture {
        let seq = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let unique = format!("mcsound-test-{}-{seq}", std::process::id());
        let root = env::temp_dir().join(unique);
        let _ = fs::remove_dir_all(&root);
        for name in names {
            let path = root.join(format!("{name}.ogg"));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create dirs");
            }
            fs::write(&path, b"ogg").expect("write file");
        }
        BundledFixture {
            assets: MinecraftAssets::Bundled { root: root.clone() },
            root,
        }
    }

    #[test]
    fn unknown_name_is_an_error() {
        let BundledFixture { assets, root } = bundled_with(&["mob/zombie/death", "block/stone/break"]);
        let err = assets
            .resolve_sound("this/does/not/exist")
            .expect_err("unknown sound must error");
        match &err {
            AssetError::UnknownSound { name, .. } => assert_eq!(name, "this/does/not/exist"),
            other => panic!("expected UnknownSound, got {other:?}"),
        }
        // Message names the input and points at `list`.
        let msg = err.to_string();
        assert!(msg.contains("this/does/not/exist"), "msg: {msg}");
        assert!(msg.contains("minecraft-sound list"), "msg: {msg}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn known_name_resolves() {
        let BundledFixture { assets, root } = bundled_with(&["mob/zombie/death"]);
        let path = assets
            .resolve_sound("mob/zombie/death")
            .expect("known sound resolves");
        assert!(path.ends_with("mob/zombie/death.ogg"), "path: {path:?}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn close_typo_is_suggested() {
        let BundledFixture { assets, root } = bundled_with(&["mob/zombie/death"]);
        let err = assets
            .resolve_sound("mob/zombie/deaht")
            .expect_err("typo must error");
        match &err {
            AssetError::UnknownSound { suggestions, .. } => {
                assert!(
                    suggestions.iter().any(|s| s == "mob/zombie/death"),
                    "suggestions: {suggestions:?}"
                );
            }
            other => panic!("expected UnknownSound, got {other:?}"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }
}
