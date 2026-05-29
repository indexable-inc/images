//! User configuration and keybind parsing.
//!
//! Config lives at `~/.config/tap/config.toml` and is optional; defaults give a
//! working session with no file. Keybinds match both legacy terminal byte
//! sequences and Kitty keyboard-protocol CSI-u, so a bind fires whether or not
//! an inner app has negotiated Kitty input on the client's terminal.

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

const DEFAULT_EDITOR_KEYBIND: &str = "Alt-e";
const DEFAULT_DETACH_KEYBIND: &str = "Ctrl-\\";
const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 50;
const DEFAULT_EDITOR: &str = "vi";

/// Top-level configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Editor for the scrollback keybind. Falls back to `$EDITOR`, `$VISUAL`,
    /// then `vi`.
    pub editor: Option<String>,
    /// Keybinds.
    pub keybinds: Keybinds,
    /// Timing knobs.
    pub timing: Timing,
}

/// Keybind strings, parsed into [`Keybind`] at load.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Keybinds {
    /// Open the scrollback in an editor.
    pub editor: String,
    /// Detach from the session.
    pub detach: String,
}

/// Timing configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Timing {
    /// Milliseconds to wait distinguishing a lone `ESC` from an `Alt-` sequence.
    pub escape_timeout_ms: u64,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            editor: DEFAULT_EDITOR_KEYBIND.to_string(),
            detach: DEFAULT_DETACH_KEYBIND.to_string(),
        }
    }
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            escape_timeout_ms: DEFAULT_ESCAPE_TIMEOUT_MS,
        }
    }
}

/// Path to the config file: `~/.config/tap/config.toml`.
#[must_use]
pub fn config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from(".config"))
        .join("tap")
        .join("config.toml")
}

/// Load configuration, falling back to defaults when no file exists.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        return Ok(Config::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("parsing config {}", path.display()))
}

/// Resolve the effective editor command.
#[must_use]
pub fn editor_command(config: &Config) -> String {
    config
        .editor
        .clone()
        .or_else(|| std::env::var("EDITOR").ok())
        .or_else(|| std::env::var("VISUAL").ok())
        .unwrap_or_else(|| DEFAULT_EDITOR.to_string())
}

/// A modifier + key combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keybind {
    /// `Alt-<char>`.
    Alt(char),
    /// `Ctrl-<char>`.
    Ctrl(char),
}

impl Keybind {
    /// Parse a string like `Alt-e`, `Ctrl-e`, or `Ctrl-\`.
    ///
    /// # Errors
    ///
    /// Returns an error for an unrecognized shape or modifier.
    pub fn parse(s: &str) -> Result<Self> {
        if s.eq_ignore_ascii_case("ctrl-\\") {
            return Ok(Self::Ctrl('\\'));
        }
        let (modifier, key) = s
            .split_once('-')
            .with_context(|| format!("invalid keybind '{s}'; expected 'Alt-<key>' or 'Ctrl-<key>'"))?;
        let key = key
            .chars()
            .next()
            .with_context(|| format!("missing key in keybind '{s}'"))?;
        match modifier.to_ascii_lowercase().as_str() {
            "alt" => Ok(Self::Alt(key)),
            "ctrl" => Ok(Self::Ctrl(key.to_ascii_lowercase())),
            other => bail!("unknown modifier '{other}'; expected 'Alt' or 'Ctrl'"),
        }
    }

    /// If `bytes` begins with this keybind, return how many bytes it consumed.
    /// Tries Kitty CSI-u first, then legacy sequences.
    #[must_use]
    pub fn matches(self, bytes: &[u8]) -> Option<usize> {
        if let Some(consumed) = self.matches_kitty(bytes) {
            return Some(consumed);
        }
        match self {
            Self::Alt(c) => {
                let byte = u8::try_from(c).ok()?;
                (bytes.len() >= 2 && bytes[0] == 0x1b && bytes[1] == byte).then_some(2)
            }
            Self::Ctrl(c) => {
                let ctrl_byte = u8::try_from(c).ok()? & 0x1f;
                (!bytes.is_empty() && bytes[0] == ctrl_byte).then_some(1)
            }
        }
    }

    /// Match a Kitty keyboard-protocol sequence `CSI <codepoint>;<modifiers> u`.
    fn matches_kitty(self, bytes: &[u8]) -> Option<usize> {
        const ALT_MODIFIER: u32 = 3;
        const CTRL_MODIFIER: u32 = 5;

        if bytes.len() < 4 || bytes[0] != 0x1b || bytes[1] != b'[' {
            return None;
        }
        let u_pos = bytes.iter().position(|&b| b == b'u')?;
        if u_pos < 3 {
            return None;
        }
        let seq = std::str::from_utf8(bytes.get(2..u_pos)?).ok()?;
        let mut parts = seq.split(';');
        let codepoint: u32 = parts.next()?.parse().ok()?;
        let modifiers: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);

        let expected = match self {
            Self::Alt(c) | Self::Ctrl(c) => u32::from(c),
        };
        if codepoint != expected {
            return None;
        }
        let wanted = match self {
            Self::Alt(_) => ALT_MODIFIER,
            Self::Ctrl(_) => CTRL_MODIFIER,
        };
        (modifiers == wanted).then_some(u_pos + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alt_and_ctrl() {
        assert_eq!(Keybind::parse("Alt-e").unwrap(), Keybind::Alt('e'));
        assert_eq!(Keybind::parse("Ctrl-c").unwrap(), Keybind::Ctrl('c'));
        assert_eq!(Keybind::parse("Ctrl-\\").unwrap(), Keybind::Ctrl('\\'));
    }

    #[test]
    fn matches_legacy_sequences() {
        assert_eq!(Keybind::Alt('e').matches(&[0x1b, b'e']), Some(2));
        assert_eq!(Keybind::Alt('e').matches(&[0x1b, b'x']), None);
        assert_eq!(Keybind::Ctrl('c').matches(&[0x03]), Some(1));
        assert_eq!(Keybind::Ctrl('\\').matches(&[0x1c]), Some(1));
    }

    #[test]
    fn matches_kitty_csi_u() {
        // Alt-e is codepoint 101 (e), modifier 3 (alt).
        assert_eq!(Keybind::Alt('e').matches(b"\x1b[101;3u"), Some(8));
        // Wrong modifier does not match.
        assert_eq!(Keybind::Alt('e').matches(b"\x1b[101;5u"), None);
    }

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.keybinds.editor, DEFAULT_EDITOR_KEYBIND);
        assert_eq!(config.keybinds.detach, DEFAULT_DETACH_KEYBIND);
        assert_eq!(config.timing.escape_timeout_ms, DEFAULT_ESCAPE_TIMEOUT_MS);
    }
}
