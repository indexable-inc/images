//! Named `Deserialize` shapes for the slices of the Slack export this adapter
//! reads.
//!
//! The export's JSON has far more fields than we use; every struct keeps only
//! what the adapter needs and uses `#[serde(default)]` so an optional field
//! absent from a given record (or a future schema change) parses rather than
//! erroring. Fields we deliberately ignore (avatars, image URLs, huddle state,
//! the heterogeneous `members` field that is sometimes an int and sometimes an
//! array) are simply not declared.

use serde::Deserialize;

/// One entry in `channels.json`.
///
/// `name` may or may not carry the channel-id prefix that the on-disk
/// directory uses; we treat the directory name as canonical for resolution and
/// use this only to fill in metadata (topic, archived flag) by id.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelEntry {
    /// Stable channel id, e.g. `C0AGC8VFVQV`.
    pub id: String,
    /// Channel name as recorded in the export.
    #[serde(default)]
    pub name: String,
    /// Whether the channel is archived.
    #[serde(default)]
    pub is_archived: bool,
    /// Channel topic; only `value` is used.
    #[serde(default)]
    pub topic: TextValue,
}

/// A `{ "value": "..." }` wrapper used by both `topic` and `purpose`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TextValue {
    /// The human text, possibly empty.
    #[serde(default)]
    pub value: String,
}

/// One entry in `users.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct UserEntry {
    /// Stable user id, e.g. `U0A5CNC980Z`.
    pub id: String,
    /// The handle (`name`), used as a last resort before the raw id.
    #[serde(default)]
    pub name: String,
    /// Whether this user is a bot/integration account.
    #[serde(default)]
    pub is_bot: bool,
    /// Profile holding the display and real names.
    #[serde(default)]
    pub profile: UserProfile,
}

/// The subset of a user (or message-embedded) profile we resolve names from.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserProfile {
    /// Preferred display name; frequently empty in exports.
    #[serde(default)]
    pub display_name: String,
    /// Fallback real name when the display name is empty.
    #[serde(default)]
    pub real_name: String,
}

impl UserProfile {
    /// The best non-empty name in this profile, if any: display name, else
    /// real name. Returns `None` when both are empty so the caller can fall
    /// through to a further fallback.
    #[must_use]
    pub fn best_name(&self) -> Option<&str> {
        let display = self.display_name.trim();
        if !display.is_empty() {
            return Some(display);
        }
        let real = self.real_name.trim();
        if real.is_empty() { None } else { Some(real) }
    }
}

/// One Slack message object from a `YYYY-MM-DD.json` day file.
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    /// Message timestamp, the per-message key, e.g. `"1775413663.871359"`.
    #[serde(default)]
    pub ts: String,
    /// Thread root timestamp; present on both replies and thread roots.
    #[serde(default)]
    pub thread_ts: Option<String>,
    /// Message subtype; `None`/absent for a regular human message.
    #[serde(default)]
    pub subtype: Option<String>,
    /// Author user id, e.g. `U0A5CNC980Z`; absent on some bot messages.
    #[serde(default)]
    pub user: Option<String>,
    /// Raw message text with Slack markup still present.
    #[serde(default)]
    pub text: String,
    /// Bot id, present on integration/bot messages.
    #[serde(default)]
    pub bot_id: Option<String>,
    /// App id, present on some app messages.
    #[serde(default)]
    pub app_id: Option<String>,
    /// Bot profile, the name source for a bot message.
    #[serde(default)]
    pub bot_profile: Option<BotProfile>,
    /// Profile embedded on the message, the fallback name source when the
    /// author id is missing from `users.json`.
    #[serde(default)]
    pub user_profile: Option<UserProfile>,
    /// Reactions on the message.
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    /// Files attached to the message.
    #[serde(default)]
    pub files: Vec<FileRef>,
    /// Message attachments (link unfurls, bot cards).
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

impl Message {
    /// The grouping key for this message: its `thread_ts` if it is part of a
    /// thread, otherwise its own `ts` (a standalone single-message thread).
    #[must_use]
    pub fn thread_key(&self) -> &str {
        match &self.thread_ts {
            Some(root) if !root.is_empty() => root,
            _ => &self.ts,
        }
    }

    /// Whether this message originates from a bot or integration.
    #[must_use]
    pub fn is_bot(&self) -> bool {
        self.bot_id.is_some() || self.app_id.is_some() || self.subtype.as_deref() == Some("bot_message")
    }
}

/// A bot's display profile on a message.
#[derive(Debug, Clone, Deserialize)]
pub struct BotProfile {
    /// The bot's display name, e.g. `Better Stack`.
    #[serde(default)]
    pub name: String,
}

/// A single reaction tally.
#[derive(Debug, Clone, Deserialize)]
pub struct Reaction {
    /// Emoji short name, e.g. `+1`.
    #[serde(default)]
    pub name: String,
    /// How many people reacted.
    #[serde(default)]
    pub count: u32,
}

/// A file attached to a message. We index its name and type, never its bytes.
#[derive(Debug, Clone, Deserialize)]
pub struct FileRef {
    /// File name, e.g. `image.png`.
    #[serde(default)]
    pub name: String,
    /// Title, used when `name` is empty.
    #[serde(default)]
    pub title: String,
    /// Human-readable type, e.g. `PNG`.
    #[serde(default)]
    pub pretty_type: String,
    /// Machine type, e.g. `png`; fallback when `pretty_type` is empty.
    #[serde(default)]
    pub filetype: String,
}

impl FileRef {
    /// The best label for the file: its name, else its title, else `"file"`.
    #[must_use]
    pub fn label(&self) -> &str {
        let name = self.name.trim();
        if !name.is_empty() {
            return name;
        }
        let title = self.title.trim();
        if title.is_empty() { "file" } else { title }
    }

    /// The best type string: `pretty_type`, else `filetype`. May be empty.
    #[must_use]
    pub fn type_str(&self) -> &str {
        let pretty = self.pretty_type.trim();
        if pretty.is_empty() { self.filetype.trim() } else { pretty }
    }
}

/// A message attachment (link unfurl or bot card). We pull whatever prose it
/// carries into the body.
#[derive(Debug, Clone, Deserialize)]
pub struct Attachment {
    /// Plain-text fallback Slack provides for the attachment.
    #[serde(default)]
    pub fallback: String,
    /// The attachment body text, when richer than the fallback.
    #[serde(default)]
    pub text: String,
    /// The attachment title.
    #[serde(default)]
    pub title: String,
}

impl Attachment {
    /// The best prose for this attachment: its text, then title, then a
    /// fallback that is not Slack's placeholder noise. Empty when nothing
    /// useful is present.
    #[must_use]
    pub fn prose(&self) -> &str {
        let text = self.text.trim();
        if !text.is_empty() {
            return text;
        }
        let title = self.title.trim();
        if !title.is_empty() {
            return title;
        }
        let fallback = self.fallback.trim();
        if fallback.is_empty() || fallback == "[no preview available]" {
            ""
        } else {
            fallback
        }
    }
}
