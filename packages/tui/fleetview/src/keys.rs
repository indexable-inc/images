//! Turn a crossterm key event back into the bytes a real terminal would have
//! sent, so an attached session behaves like the agent's own terminal.
//!
//! Only the encoding lives here. Cursor keys are emitted in their normal CSI
//! form; the `tui` crate rewrites them to the SS3 form when the child has
//! enabled DECCKM, so a full-screen agent still receives its arrows.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The bytes for `key`, or `None` for a key with no terminal representation.
pub fn encode(key: KeyEvent) -> Option<String> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let body = body(key)?;
    // Alt is the ESC prefix, exactly as a terminal sends it.
    Some(if alt { format!("\x1b{body}") } else { body })
}

fn body(key: KeyEvent) -> Option<String> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char(c) if ctrl => control(c).map(String::from),
        KeyCode::Char(c) => Some(c.to_string()),
        // Shift+Enter is how Claude Code takes a newline without submitting;
        // terminals spell that ESC-Enter, the same as Alt+Enter.
        KeyCode::Enter if shift => Some("\x1b\r".to_owned()),
        KeyCode::Enter => Some("\r".to_owned()),
        KeyCode::Tab => Some("\t".to_owned()),
        KeyCode::BackTab => Some("\x1b[Z".to_owned()),
        KeyCode::Backspace => Some("\x7f".to_owned()),
        KeyCode::Esc => Some("\x1b".to_owned()),
        KeyCode::Up => Some("\x1b[A".to_owned()),
        KeyCode::Down => Some("\x1b[B".to_owned()),
        KeyCode::Right => Some("\x1b[C".to_owned()),
        KeyCode::Left => Some("\x1b[D".to_owned()),
        KeyCode::Home => Some("\x1b[H".to_owned()),
        KeyCode::End => Some("\x1b[F".to_owned()),
        KeyCode::PageUp => Some("\x1b[5~".to_owned()),
        KeyCode::PageDown => Some("\x1b[6~".to_owned()),
        KeyCode::Insert => Some("\x1b[2~".to_owned()),
        KeyCode::Delete => Some("\x1b[3~".to_owned()),
        KeyCode::F(n) => function(n),
        _ => None,
    }
}

/// The control character for `c`, following the ASCII `@`..`_` block that
/// Ctrl folds onto.
fn control(c: char) -> Option<char> {
    let upper = c.to_ascii_uppercase();
    match upper {
        ' ' | '@' => Some('\0'),
        '?' => Some('\x7f'),
        '@'..='_' => char::from_u32(u32::from(upper) - 64),
        _ => None,
    }
}

/// The xterm sequence for a function key.
fn function(n: u8) -> Option<String> {
    match n {
        1 => Some("\x1bOP".to_owned()),
        2 => Some("\x1bOQ".to_owned()),
        3 => Some("\x1bOR".to_owned()),
        4 => Some("\x1bOS".to_owned()),
        5 => Some("\x1b[15~".to_owned()),
        6..=8 => Some(format!("\x1b[{}~", n + 11)),
        9..=10 => Some(format!("\x1b[{}~", n + 12)),
        11..=12 => Some(format!("\x1b[{}~", n + 13)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::encode;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
        encode(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn plain_characters_pass_through() {
        assert_eq!(
            key(KeyCode::Char('a'), KeyModifiers::NONE).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn control_folds_onto_the_ascii_block() {
        assert_eq!(
            key(KeyCode::Char('a'), KeyModifiers::CONTROL).as_deref(),
            Some("\x01")
        );
        assert_eq!(
            key(KeyCode::Char('c'), KeyModifiers::CONTROL).as_deref(),
            Some("\x03")
        );
        assert_eq!(
            key(KeyCode::Char(' '), KeyModifiers::CONTROL).as_deref(),
            Some("\0")
        );
    }

    #[test]
    fn alt_is_the_escape_prefix() {
        assert_eq!(
            key(KeyCode::Char('b'), KeyModifiers::ALT).as_deref(),
            Some("\x1bb")
        );
    }

    #[test]
    fn enter_submits_and_shift_enter_inserts_a_newline() {
        assert_eq!(
            key(KeyCode::Enter, KeyModifiers::NONE).as_deref(),
            Some("\r")
        );
        assert_eq!(
            key(KeyCode::Enter, KeyModifiers::SHIFT).as_deref(),
            Some("\x1b\r")
        );
    }

    #[test]
    fn arrows_use_the_normal_csi_form() {
        assert_eq!(
            key(KeyCode::Up, KeyModifiers::NONE).as_deref(),
            Some("\x1b[A")
        );
        assert_eq!(
            key(KeyCode::Left, KeyModifiers::NONE).as_deref(),
            Some("\x1b[D")
        );
    }

    #[test]
    fn a_key_with_no_terminal_form_encodes_to_nothing() {
        assert_eq!(key(KeyCode::CapsLock, KeyModifiers::NONE), None);
    }
}
