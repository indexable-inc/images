//! Rendering Slack message text into the plain prose that gets embedded.
//!
//! Slack stores entities as angle-bracket tokens (`<@U123>`, `<#C1|name>`,
//! `<url|label>`, `<!here>`) and HTML-escapes `&`, `<`, `>`. This module turns
//! those into readable text before the body is hashed and embedded. It scans
//! the string once rather than pulling in a regex engine; the grammar is small
//! and unnested.
//!
//! Mentions are resolved through the [`UserMap`], so a `<@U123>` becomes
//! `@Display Name`. The id is never dropped: an unknown id renders as `@U123`.

use crate::{model::UserProfile, users::UserMap};

/// Render one message's raw text into embeddable prose.
///
/// `users` resolves `<@U..>` mentions; `message_profile` is the author's
/// message-embedded profile, used as a mention fallback for the author's own
/// id when it is missing from the users map.
#[must_use]
pub fn render_text(raw: &str, users: &UserMap, message_profile: Option<&UserProfile>) -> String {
    let expanded = expand_entities(raw, users, message_profile);
    let unescaped = unescape_html(&expanded);
    collapse_blank_lines(&unescaped)
}

/// Walk the string once, replacing each `<...>` entity. Text outside brackets
/// is copied verbatim, so `*bold*` and `` `code` `` survive as literal words.
fn expand_entities(raw: &str, users: &UserMap, message_profile: Option<&UserProfile>) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('>') else {
            // Unterminated `<`: keep it literally and stop scanning entities.
            out.push('<');
            out.push_str(after_open);
            return out;
        };
        let token = &after_open[..close];
        out.push_str(&render_entity(token, users, message_profile));
        rest = &after_open[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Render the inside of one `<...>` token (the angle brackets already stripped).
fn render_entity(token: &str, users: &UserMap, message_profile: Option<&UserProfile>) -> String {
    if let Some(rest) = token.strip_prefix('@') {
        // User mention: `@U123` or `@U123|label`.
        let id = rest.split('|').next().unwrap_or(rest);
        return format!("@{}", users.resolve(id, message_profile));
    }
    if let Some(rest) = token.strip_prefix('#') {
        // Channel mention: `#C123|name` -> `#name`; `#C123` -> `#C123`.
        return match rest.split_once('|') {
            Some((_id, name)) if !name.is_empty() => format!("#{name}"),
            _ => format!("#{rest}"),
        };
    }
    if let Some(rest) = token.strip_prefix('!') {
        // Broadcast: `!here`, `!channel`, `!everyone`, or `!subteam^..|@grp`.
        return render_broadcast(rest);
    }
    // Link: `url|label` -> `label`; bare `url` -> `url`.
    match token.split_once('|') {
        Some((_url, label)) if !label.is_empty() => label.to_owned(),
        Some((url, _)) => url.to_owned(),
        None => token.to_owned(),
    }
}

/// Render a `<!...>` broadcast token to an `@`-prefixed word.
fn render_broadcast(rest: &str) -> String {
    let keyword = match rest.split_once('|') {
        // `subteam^S123|@group` -> keep the human label after the pipe.
        Some((_id, label)) if !label.is_empty() => {
            return label.to_owned();
        }
        Some((id, _)) => id,
        None => rest,
    };
    let word = keyword.split('^').next().unwrap_or(keyword);
    format!("@{word}")
}

/// Reverse Slack's HTML escaping of `&`, `<`, `>`.
///
/// Order matters: `&amp;` is decoded last so an escaped `&lt;` cannot be
/// double-decoded into a real `<`.
fn unescape_html(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Collapse any run of three or more consecutive newlines down to two, so a
/// message body never has more than one blank line in a row.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newline_run: usize = 0;
    for ch in text.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push('\n');
            }
        } else {
            newline_run = 0;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::render_text;
    use crate::{model::UserEntry, users::UserMap};

    fn user_map() -> UserMap {
        let json = serde_json::json!([
            { "id": "U0A5CNC980Z", "name": "andrew",
              "profile": { "display_name": "andrew", "real_name": "Andrew Gazelka" } },
            { "id": "U0AH08A8V5G", "name": "wyatt",
              "profile": { "display_name": "", "real_name": "wyatt" } },
        ]);
        let entries: Vec<UserEntry> = serde_json::from_value(json).expect("parse fixture users");
        UserMap::from_entries(entries)
    }

    #[test]
    fn resolves_user_mention_to_display_name() {
        let out = render_text("hey <@U0A5CNC980Z> look", &user_map(), None);
        assert_eq!(out, "hey @andrew look");
    }

    #[test]
    fn falls_back_to_real_name_then_id() {
        assert_eq!(render_text("<@U0AH08A8V5G>", &user_map(), None), "@wyatt");
        assert_eq!(render_text("<@U0UNKNOWN1>", &user_map(), None), "@U0UNKNOWN1");
    }

    #[test]
    fn renders_channel_and_link_and_broadcast() {
        let out = render_text("see <#C0X|craft> at <https://ix.dev|ix> <!here>", &user_map(), None);
        assert_eq!(out, "see #craft at ix @here");
    }

    #[test]
    fn keeps_bare_link_and_markup_literals() {
        let out = render_text("read <https://ix.dev> with *bold* `code`", &user_map(), None);
        assert_eq!(out, "read https://ix.dev with *bold* `code`");
    }

    #[test]
    fn unescapes_html_entities() {
        assert_eq!(render_text("sdk &gt;&gt;&gt; cli &amp; more", &user_map(), None), "sdk >>> cli & more");
    }

    #[test]
    fn collapses_three_or_more_blank_lines() {
        assert_eq!(render_text("a\n\n\n\nb", &user_map(), None), "a\n\nb");
    }
}
