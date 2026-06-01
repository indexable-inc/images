//! Assembling a channel's messages into per-thread documents.
//!
//! A thread is `(channel_id, thread_ts)` where `thread_ts` is the root ts; a
//! standalone message keys on its own ts. Replies of one thread can be
//! scattered across several `YYYY-MM-DD.json` files, so the caller must hand
//! this module every message in the channel directory at once; we group across
//! all of them, sort each thread by ts, and emit one [`Document`] per thread.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use source_meta::{Document, hash_body, keys};
use serde_json::{Map, Value, json};
use snafu::ResultExt as _;

use crate::{
    channel::ChannelInfo,
    error::{Error, MetadataSnafu},
    model::Message,
    render::render_text,
    users::UserMap,
};

/// MIME type chosen for the rendered thread body: it is plain prose.
const BODY_MIME: &str = "text/plain; charset=utf-8";

/// Subtypes that are administrative / system noise and never carry thread
/// content. Messages with one of these are dropped before grouping.
const DROP_SUBTYPES: &[&str] = &[
    "channel_join",
    "channel_leave",
    "channel_name",
    "channel_archive",
    "channel_topic",
    "channel_purpose",
    "channel_convert_to_public",
    "channel_convert_to_private",
    "bot_add",
    "bot_remove",
    "reminder_add",
    "automatic_ai_huddle_notes_enabled",
    "huddle_thread",
];

/// Whether a message should be kept for embedding.
///
/// Drops system/admin subtypes and any message with no usable ts (it cannot be
/// keyed or ordered). `thread_broadcast` is kept: it is a real reply that was
/// also broadcast to the channel, and grouping by `thread_ts` plus ts-dedup
/// makes it a single thread member rather than a duplicate standalone.
#[must_use]
fn is_content_message(message: &Message) -> bool {
    if message.ts.is_empty() {
        return false;
    }
    message
        .subtype
        .as_deref()
        .is_none_or(|subtype| !DROP_SUBTYPES.contains(&subtype))
}

/// Parse the integer (epoch-seconds) part of a Slack `ts` string like
/// `"1775413663.871359"`. Returns `None` when there is no leading integer.
#[must_use]
fn ts_epoch_seconds(ts: &str) -> Option<i64> {
    let seconds = ts.split('.').next().unwrap_or(ts);
    seconds.parse::<i64>().ok()
}

/// Format epoch seconds as a `YYYY-MM-DD` UTC date without pulling in a date
/// crate: a plain civil-calendar conversion of the day number.
#[must_use]
fn iso_date(epoch_seconds: i64) -> String {
    // Days since the Unix epoch (floor division so pre-1970 would still work).
    let days = epoch_seconds.div_euclid(86_400);
    // Convert a day number to a Gregorian date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// Build every thread document for one channel from all its messages.
///
/// Messages may arrive in any order and from any day file. They are filtered,
/// grouped by thread key, each thread sorted by ts and de-duplicated by ts
/// (handling `thread_broadcast`), then rendered. Threads are emitted in
/// ascending root-ts order for deterministic output.
///
/// # Errors
/// Returns [`Error::Metadata`] if a thread's flattened metadata exceeds a store
/// limit.
pub fn documents_for_channel(
    channel: &ChannelInfo,
    users: &UserMap,
    messages: Vec<Message>,
) -> Result<Vec<Document>, Error> {
    // Group by thread key. BTreeMap keeps roots in ascending ts order; the
    // inner BTreeMap keys by ts so duplicates (broadcasts re-seen) collapse and
    // members come out time-ordered.
    let mut threads: BTreeMap<String, BTreeMap<String, Message>> = BTreeMap::new();
    for message in messages {
        if !is_content_message(&message) {
            continue;
        }
        let key = message.thread_key().to_owned();
        threads.entry(key).or_default().insert(message.ts.clone(), message);
    }

    let mut documents = Vec::with_capacity(threads.len());
    for (thread_ts, members) in threads {
        let ordered: Vec<Message> = members.into_values().collect();
        if ordered.is_empty() {
            continue;
        }
        documents.push(build_document(channel, users, &thread_ts, &ordered)?);
    }
    Ok(documents)
}

/// Build one thread's [`Document`] from its time-ordered members.
fn build_document(
    channel: &ChannelInfo,
    users: &UserMap,
    thread_ts: &str,
    ordered: &[Message],
) -> Result<Document, Error> {
    let root = &ordered[0];
    let root_epoch = ts_epoch_seconds(&root.ts);

    let mut authors: Vec<String> = Vec::new();
    let mut has_files = false;
    let mut all_bot = true;

    let mut body = String::new();
    write_header(&mut body, channel, root_epoch, ordered, users, &mut authors);

    for message in ordered {
        let bot = message.is_bot() || message.user.as_deref().is_some_and(|id| users.is_bot(id));
        if !bot {
            all_bot = false;
        }
        if !message.files.is_empty() {
            has_files = true;
        }
        write_message_block(&mut body, message, users, bot);
    }

    let external_id = format!("slack:{}:{thread_ts}", channel.id);
    let body_bytes = body.into_bytes();
    let content_hash = hash_body(&body_bytes);

    let title = thread_title(channel, root, users);
    let meta = build_meta(&BuildMeta {
        external_id: &external_id,
        content_hash: &content_hash,
        title: &title,
        timestamp: root_epoch,
        channel,
        authors: &authors,
        is_bot_thread: all_bot,
        message_count: ordered.len(),
        has_files,
        thread_ts,
    });

    source_meta::check_metadata(&external_id, &meta).context(MetadataSnafu {
        external_id: external_id.clone(),
    })?;

    Ok(Document {
        external_id,
        file_name: format!("slack/{}/{thread_ts}.txt", channel.id),
        mime: BODY_MIME,
        body: body_bytes,
        meta_json: meta,
        content_hash,
    })
}

/// Write the channel/date/participants header into `body` and collect the
/// distinct resolved author names (in first-seen order) into `authors`.
fn write_header(
    body: &mut String,
    channel: &ChannelInfo,
    root_epoch: Option<i64>,
    ordered: &[Message],
    users: &UserMap,
    authors: &mut Vec<String>,
) {
    for message in ordered {
        let name = author_name(message, users).to_owned();
        if !authors.iter().any(|existing| existing == &name) {
            authors.push(name);
        }
    }

    body.push_str("Channel: #");
    body.push_str(&channel.name);
    if channel.is_archived {
        body.push_str(" (archived)");
    }
    body.push('\n');

    if let Some(epoch) = root_epoch {
        body.push_str("Date: ");
        body.push_str(&iso_date(epoch));
        body.push('\n');
    }

    body.push_str("Participants: ");
    body.push_str(&authors.join(", "));
    body.push('\n');

    let topic = channel.topic.trim();
    if !topic.is_empty() {
        body.push_str("Topic: ");
        body.push_str(topic);
        body.push('\n');
    }
    body.push('\n');
}

/// Write one message block (author line, rendered text, files, attachment
/// prose, reactions) into `body`.
fn write_message_block(body: &mut String, message: &Message, users: &UserMap, bot: bool) {
    let author = author_name(message, users);
    body.push('[');
    body.push_str(author);
    if bot {
        body.push_str(" (bot)");
    }
    body.push_str("] ");

    let rendered = render_text(&message.text, users, message.user_profile.as_ref());
    body.push_str(rendered.trim_end());
    body.push('\n');

    for file in &message.files {
        // Infallible writes into a `String`.
        let type_str = file.type_str();
        if type_str.is_empty() {
            let _ = writeln!(body, "  attached: {}", file.label());
        } else {
            let _ = writeln!(body, "  attached: {} ({type_str})", file.label());
        }
    }

    // Attachment prose (link unfurls, bot cards) is the only text source when a
    // bot message has no top-level text, and supplements it otherwise.
    for attachment in &message.attachments {
        let prose = attachment.prose();
        if !prose.is_empty() {
            let rendered_prose = render_text(prose, users, message.user_profile.as_ref());
            for line in rendered_prose.lines() {
                let line = line.trim_end();
                if !line.is_empty() {
                    body.push_str("  ");
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }

    if !message.reactions.is_empty() {
        let parts: Vec<String> = message
            .reactions
            .iter()
            .map(|reaction| format!("{}×{}", reaction.name, reaction.count))
            .collect();
        body.push_str("  reactions: ");
        body.push_str(&parts.join(", "));
        body.push('\n');
    }
}

/// The display name for a message's author, following the resolution chain.
///
/// For a human message the author is `user`; for a bot message with no `user`
/// we prefer the bot profile name, then fall back to the embedded profile, then
/// a generic `bot` label.
fn author_name<'a>(message: &'a Message, users: &'a UserMap) -> &'a str {
    if let Some(id) = message.user.as_deref().filter(|id| !id.is_empty()) {
        return users.resolve(id, message.user_profile.as_ref());
    }
    if let Some(name) = message
        .bot_profile
        .as_ref()
        .map(|profile| profile.name.trim())
        .filter(|name| !name.is_empty())
    {
        return name;
    }
    if let Some(name) = message.user_profile.as_ref().and_then(crate::model::UserProfile::best_name) {
        return name;
    }
    "bot"
}

/// A short human title for the thread: `#channel: first non-empty rendered
/// line`, truncated to a sane length.
fn thread_title(channel: &ChannelInfo, root: &Message, users: &UserMap) -> String {
    let rendered = render_text(&root.text, users, root.user_profile.as_ref());
    let snippet = rendered
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("(no text)");
    let snippet = truncate_chars(snippet, 80);
    format!("#{}: {snippet}", channel.name)
}

/// Truncate to at most `max` characters on a char boundary, appending an
/// ellipsis when shortened.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// Arguments for [`build_meta`], grouped to keep the call site readable and
/// satisfy the clippy `too_many_arguments` lint.
struct BuildMeta<'a> {
    external_id: &'a str,
    content_hash: &'a str,
    title: &'a str,
    timestamp: Option<i64>,
    channel: &'a ChannelInfo,
    authors: &'a [String],
    is_bot_thread: bool,
    message_count: usize,
    has_files: bool,
    thread_ts: &'a str,
}

/// Build the flat metadata object: the common header keys plus the Slack
/// extras. Every key is top-level so each is a filter key.
fn build_meta(args: &BuildMeta<'_>) -> Value {
    let mut map = Map::new();
    map.insert(keys::SOURCE.to_owned(), json!(source_meta::Source::new("slack").as_str()));
    map.insert("external_id".to_owned(), json!(args.external_id));
    map.insert(keys::CONTENT_HASH.to_owned(), json!(args.content_hash));
    map.insert(keys::TITLE.to_owned(), json!(args.title));
    if let Some(timestamp) = args.timestamp {
        map.insert(keys::TIMESTAMP.to_owned(), json!(timestamp));
    }
    map.insert(keys::CHANNEL_ID.to_owned(), json!(args.channel.id));
    map.insert(keys::CHANNEL_NAME.to_owned(), json!(args.channel.name));
    map.insert(keys::AUTHORS.to_owned(), json!(args.authors));
    map.insert(keys::IS_ARCHIVED.to_owned(), json!(args.channel.is_archived));
    map.insert(keys::IS_EXTERNAL.to_owned(), json!(args.channel.is_external));
    map.insert(keys::IS_BOT_THREAD.to_owned(), json!(args.is_bot_thread));
    map.insert("message_count".to_owned(), json!(args.message_count));
    map.insert("has_files".to_owned(), json!(args.has_files));
    map.insert("thread_ts".to_owned(), json!(args.thread_ts));
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::{iso_date, ts_epoch_seconds};

    #[test]
    fn parses_ts_epoch_seconds() {
        assert_eq!(ts_epoch_seconds("1775413663.871359"), Some(1_775_413_663));
        assert_eq!(ts_epoch_seconds("1775413663"), Some(1_775_413_663));
        assert_eq!(ts_epoch_seconds("nope"), None);
    }

    #[test]
    fn formats_iso_date_utc() {
        // 2026-04-05 (a ts seen in the real export).
        assert_eq!(iso_date(1_775_413_663), "2026-04-05");
        // Unix epoch day zero.
        assert_eq!(iso_date(0), "1970-01-01");
    }
}
