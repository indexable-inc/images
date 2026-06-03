//! Text-art "avatars" for the status line. Claude Code strips Kitty graphics
//! APC sequences from status-line output (`anthropics/claude-code#39024`), so real
//! profile images are impossible there. Instead each avatar is two half-circle
//! glyphs (◖ ◗) flanking the author's initials, colored along the Instagram
//! story gradient (purple -> pink -> orange) so the row reads as ringed avatars.

use crate::story::Story;

type Rgb = (u8, u8, u8);

/// Instagram story-ring gradient stops.
const GRADIENT: [Rgb; 5] = [
    (0x83, 0x3A, 0xB4),
    (0xC1, 0x35, 0x84),
    (0xE1, 0x30, 0x6C),
    (0xFC, 0xAF, 0x45),
    (0xF7, 0x77, 0x37),
];

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

// The result is a rounded color component clamped to 0..=255, so the byte cast
// cannot lose information. Same idiom as git-log-pretty's palette.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (f32::from(b) - f32::from(a))
        .mul_add(t, f32::from(a))
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Sample the gradient at `t` in `[0, 1]`.
// `t` is clamped to 0..=1 and GRADIENT is tiny, so the index and float casts are
// bounded and stay within the array.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]
fn grad(t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let span = t * (GRADIENT.len() - 1) as f32;
    let i = span.floor() as usize;
    if i >= GRADIENT.len() - 1 {
        return GRADIENT[GRADIENT.len() - 1];
    }
    let f = span - i as f32;
    let (a, b) = (GRADIENT[i], GRADIENT[i + 1]);
    (lerp(a.0, b.0, f), lerp(a.1, b.1, f), lerp(a.2, b.2, f))
}

fn fg((r, g, b): Rgb) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

/// Up to two uppercase initials from a display name.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|s| !s.is_empty())
        .collect();
    let pick = |s: &str| s.chars().next().map(|c| c.to_ascii_uppercase());
    let two: String = match words.as_slice() {
        [a, b, ..] => [pick(a), pick(b)].into_iter().flatten().collect(),
        [a] => a.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase(),
        [] => String::new(),
    };
    if two.is_empty() { "??".to_owned() } else { two }
}

/// A peer-supplied URL is safe to emit as a hyperlink only if it is plain
/// http(s) and carries no control characters. Stories come from untrusted peer
/// JSON, so an unguarded `url` would let any tailnet host inject terminal escape
/// sequences into the victim's status line via the OSC 8 payload.
fn is_safe_url(u: &str) -> bool {
    (u.starts_with("https://") || u.starts_with("http://")) && !u.chars().any(char::is_control)
}

/// Wrap `text` in an OSC 8 hyperlink when a safe target is present.
fn link(text: &str, url: Option<&str>) -> String {
    url.filter(|u| is_safe_url(u)).map_or_else(
        || text.to_owned(),
        |u| format!("\x1b]8;;{u}\x1b\\{text}\x1b]8;;\x1b\\"),
    )
}

/// A single ringed avatar: ◖INITIALS◗ in gradient colors.
fn avatar(story: &Story) -> String {
    let left = fg(grad(0.15));
    let mid = fg(grad(0.5));
    let right = fg(grad(0.85));
    let ini = initials(&story.name);
    let token = format!("{left}◖{mid}{BOLD}{ini}{RESET}{right}◗{RESET}");
    link(&token, story.url.as_deref())
}

/// The leading "your story" bubble: a dim ◖+◗.
fn add_bubble() -> String {
    format!("{DIM}◖{RESET}{BOLD}+{RESET}{DIM}◗{RESET}")
}

/// Render the full status-line row from the peer stories (already filtered to
/// the fresh ones). Newest first.
pub fn row(mut stories: Vec<Story>) -> String {
    stories.sort_by_key(|s| std::cmp::Reverse(s.ts));
    let label = format!("{}{BOLD}📸 STORIES{RESET}", fg(grad(0.5)));
    let mut parts = vec![label, add_bubble()];
    parts.extend(stories.iter().map(avatar));
    parts.join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_handles_common_shapes() {
        assert_eq!(initials("Andrew Gazelka"), "AG");
        assert_eq!(initials("wyatt-gill"), "WG");
        assert_eq!(initials("amber"), "AM");
        assert_eq!(initials(""), "??");
    }

    #[test]
    fn gradient_endpoints_are_stable() {
        assert_eq!(grad(0.0), GRADIENT[0]);
        assert_eq!(grad(1.0), GRADIENT[GRADIENT.len() - 1]);
    }

    #[test]
    fn unsafe_peer_urls_are_not_linked() {
        // A control char (here ESC) or a non-http scheme must never reach the
        // OSC 8 payload, or a peer could inject escapes into the status line.
        assert!(!link("X", Some("https://x\x1b]8;;evil")).contains("evil"));
        assert!(!link("X", Some("javascript:alert(1)")).contains("alert"));
        assert_eq!(link("X", None), "X");
        assert!(link("X", Some("https://github.com/a")).contains("github.com/a"));
    }

    #[test]
    fn row_contains_label_and_initials() {
        let s = Story {
            name: "Andrew Gazelka".into(),
            repo: "index".into(),
            branch: "main".into(),
            subject: "x".into(),
            ts: 0,
            url: None,
        };
        let out = row(vec![s]);
        assert!(out.contains("STORIES"));
        assert!(out.contains("AG"));
    }
}
