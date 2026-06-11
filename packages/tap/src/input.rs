//! Client-side input processing: detach and editor keybind detection.
//!
//! The attach client passes keystrokes straight through to the session except
//! for its own keybinds. A lone `ESC` is held briefly so an `Alt-` sequence that
//! arrives as `ESC` then a letter can be recognized; if nothing follows before
//! the timeout, the `ESC` is released unchanged.

use anyhow::Result;

use crate::config::{Config, Keybind};

const ESC: u8 = 0x1b;

/// What to do with a chunk of input.
#[derive(Debug)]
pub enum InputResult {
    /// Forward these bytes to the session.
    Passthrough(Vec<u8>),
    /// A keybind fired.
    Action(KeybindAction),
    /// Hold for more input or the escape timeout.
    NeedMore,
}

/// A bound action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindAction {
    /// Open the scrollback in an editor.
    OpenEditor,
    /// Detach from the session.
    Detach,
}

/// Keybind-detecting input state machine.
pub struct InputProcessor {
    binds: Vec<(Keybind, KeybindAction)>,
    escape_timeout: std::time::Duration,
    pending_escape: bool,
}

impl InputProcessor {
    /// Build from config, parsing the keybind strings.
    ///
    /// # Errors
    ///
    /// Returns an error if a configured keybind string does not parse.
    pub fn new(config: &Config) -> Result<Self> {
        let binds = vec![
            (
                Keybind::parse(&config.keybinds.editor)?,
                KeybindAction::OpenEditor,
            ),
            (
                Keybind::parse(&config.keybinds.detach)?,
                KeybindAction::Detach,
            ),
        ];
        Ok(Self {
            binds,
            escape_timeout: std::time::Duration::from_millis(config.timing.escape_timeout_ms),
            pending_escape: false,
        })
    }

    /// How long to wait for an `Alt-` sequence after a lone `ESC`.
    #[must_use]
    pub const fn escape_timeout(&self) -> std::time::Duration {
        self.escape_timeout
    }

    /// Whether a lone `ESC` is currently being held.
    #[must_use]
    pub const fn has_pending_escape(&self) -> bool {
        self.pending_escape
    }

    /// Process a chunk of input bytes.
    pub fn process(&mut self, bytes: &[u8]) -> InputResult {
        if bytes.is_empty() {
            return if self.take_pending_escape() {
                InputResult::Passthrough(vec![ESC])
            } else {
                InputResult::Passthrough(vec![])
            };
        }

        let effective = if self.take_pending_escape() {
            let mut v = Vec::with_capacity(bytes.len() + 1);
            v.push(ESC);
            v.extend_from_slice(bytes);
            v
        } else {
            bytes.to_vec()
        };

        for (bind, action) in &self.binds {
            if bind.matches(&effective).is_some() {
                return InputResult::Action(*action);
            }
        }

        if effective == [ESC] {
            self.pending_escape = true;
            return InputResult::NeedMore;
        }

        InputResult::Passthrough(effective)
    }

    /// Release a held `ESC` when its timeout expires.
    pub fn timeout_escape(&mut self) -> InputResult {
        if self.take_pending_escape() {
            InputResult::Passthrough(vec![ESC])
        } else {
            InputResult::Passthrough(vec![])
        }
    }

    fn take_pending_escape(&mut self) -> bool {
        std::mem::take(&mut self.pending_escape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn processor() -> InputProcessor {
        InputProcessor::new(&Config::default()).unwrap()
    }

    #[test]
    fn passes_normal_input_through() {
        match processor().process(b"hello") {
            InputResult::Passthrough(bytes) => assert_eq!(bytes, b"hello"),
            other => panic!("expected passthrough, got {other:?}"),
        }
    }

    #[test]
    fn lone_escape_is_held_then_released() {
        let mut p = processor();
        assert!(matches!(p.process(&[ESC]), InputResult::NeedMore));
        assert!(p.has_pending_escape());
        match p.timeout_escape() {
            InputResult::Passthrough(bytes) => assert_eq!(bytes, vec![ESC]),
            other => panic!("expected ESC release, got {other:?}"),
        }
        assert!(!p.has_pending_escape());
    }

    #[test]
    fn alt_e_fires_editor_across_split_reads() {
        let mut p = processor();
        assert!(matches!(p.process(&[ESC]), InputResult::NeedMore));
        assert!(matches!(
            p.process(b"e"),
            InputResult::Action(KeybindAction::OpenEditor)
        ));
    }

    #[test]
    fn ctrl_backslash_detaches() {
        assert!(matches!(
            processor().process(&[0x1c]),
            InputResult::Action(KeybindAction::Detach)
        ));
    }

    #[test]
    fn escape_then_other_key_passes_both() {
        let mut p = processor();
        assert!(matches!(p.process(&[ESC]), InputResult::NeedMore));
        match p.process(b"x") {
            InputResult::Passthrough(bytes) => assert_eq!(bytes, vec![ESC, b'x']),
            other => panic!("expected passthrough, got {other:?}"),
        }
    }
}
