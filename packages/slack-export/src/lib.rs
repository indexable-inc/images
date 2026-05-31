//! Turn a Slack export directory into per-thread embeddable [`Document`]s for
//! the multi-source `search` tool.
//!
//! # Grain
//!
//! One [`Document`] per **thread**. A thread is every message sharing a
//! `(channel_id, thread_ts)`; a standalone message (no `thread_ts`) is its own
//! single-message thread keyed on its `ts`. A thread's replies can be scattered
//! across several `YYYY-MM-DD.json` day files, so the adapter reads a whole
//! channel directory before grouping. Administrative/system messages
//! (`channel_join`, `channel_leave`, and similar) are dropped.
//!
//! # Streaming
//!
//! [`SlackExport::open`] reads only the small `channels.json` and `users.json`
//! up front. [`SlackExport::documents`](search_meta::SourceAdapter::documents)
//! then iterates **channel by channel**: it loads one channel's day files,
//! assembles and yields that channel's thread documents, and only then moves to
//! the next channel. The whole 344 MB export is never held in memory at once.
//!
//! # Identity and hashing
//!
//! - `external_id = "slack:{channel_id}:{thread_ts}"`.
//! - `content_hash = search_meta::hash_body(body)` over the exact embedded
//!   bytes, so re-ingesting an unchanged export is a no-op and a record
//!   re-embeds only when its rendered body actually changes.
//! - `timestamp` is the root message's `ts` integer part, epoch seconds.
//!
//! The flat `meta_json` carries the common header (`source`, `external_id`,
//! `content_hash`, `title`, `timestamp`) plus the Slack extras (`channel_id`,
//! `channel_name`, `authors`, `is_archived`, `is_external`, `is_bot_thread`,
//! `message_count`, `has_files`, `thread_ts`). Each is a top-level filter key.

mod channel;
mod error;
mod model;
mod render;
mod thread;
mod users;

use std::{collections::HashMap, fs, path::Path};

use search_meta::{Document, Source, SourceAdapter};
use snafu::ResultExt as _;

pub use crate::error::Error;
use crate::{
    channel::{ChannelDir, ChannelInfo, discover_channel_dirs, read_channel_messages, resolve_channel},
    error::{ParseSnafu, ReadSnafu},
    model::{ChannelEntry, UserEntry},
    thread::documents_for_channel,
    users::UserMap,
};

/// Default name of the channel-metadata file at the export root.
const CHANNELS_FILE: &str = "channels.json";
/// Default name of the users-map file at the export root.
const USERS_FILE: &str = "users.json";

/// An opened Slack export ready to stream into per-thread documents.
///
/// Holds only the small in-memory indices (channel metadata, the user map, the
/// list of channel directories); message bodies are read lazily, one channel at
/// a time, when [`documents`](SourceAdapter::documents) is iterated.
#[derive(Debug)]
#[must_use]
pub struct SlackExport {
    channel_dirs: Vec<ChannelDir>,
    by_id: HashMap<String, ChannelEntry>,
    by_name: HashMap<String, ChannelEntry>,
    users: UserMap,
}

impl SlackExport {
    /// Open an export rooted at `dir`, reading `channels.json` and `users.json`
    /// and listing the channel directories.
    ///
    /// `users.json` is optional: when it is absent the adapter falls back to the
    /// `user_profile` embedded on each message for name resolution.
    ///
    /// # Errors
    /// Returns [`Error::Read`] / [`Error::Parse`] if `channels.json` (or a
    /// present `users.json`) cannot be read or parsed, or [`Error::ListDir`] if
    /// the export root cannot be listed.
    pub fn open(dir: &Path) -> Result<Self, Error> {
        let channels = read_channels(&dir.join(CHANNELS_FILE))?;
        let mut by_id = HashMap::with_capacity(channels.len());
        let mut by_name = HashMap::with_capacity(channels.len());
        for entry in channels {
            by_name.entry(entry.name.clone()).or_insert_with(|| entry.clone());
            by_id.insert(entry.id.clone(), entry);
        }

        let users = read_users(&dir.join(USERS_FILE))?;
        let channel_dirs = discover_channel_dirs(dir)?;

        Ok(Self {
            channel_dirs,
            by_id,
            by_name,
            users,
        })
    }

    /// Resolve one channel directory to its full [`ChannelInfo`].
    fn channel_info(&self, channel_dir: &ChannelDir) -> ChannelInfo {
        resolve_channel(channel_dir, &self.by_id, &self.by_name)
    }
}

impl SourceAdapter for SlackExport {
    type Error = Error;

    fn source(&self) -> Source {
        Source::Slack
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Each channel's day files are read only when its turn comes, then the
        // documents for that channel are flattened out before the next channel
        // is touched. The owned `users` clone lets the streaming iterator hold
        // the user map without re-borrowing `self` per channel.
        let users = self.users.clone();

        self.channel_dirs.iter().flat_map(move |channel_dir| {
            let info = self.channel_info(channel_dir);
            let result = read_channel_messages(&info.dir)
                .and_then(|messages| documents_for_channel(&info, &users, messages));
            match result {
                Ok(documents) => DocumentBatch::Items(documents.into_iter()),
                Err(error) => DocumentBatch::Error(Some(error)),
            }
        })
    }
}

/// One channel's yield: either its documents or the single error that aborted
/// the channel. Keeping this as a concrete `Send` iterator avoids boxing while
/// still letting one channel's failure surface as a typed `Err`.
enum DocumentBatch {
    /// The channel's successfully built documents.
    Items(std::vec::IntoIter<Document>),
    /// The channel failed; yield the error exactly once.
    Error(Option<Error>),
}

impl Iterator for DocumentBatch {
    type Item = Result<Document, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Items(items) => items.next().map(Ok),
            Self::Error(slot) => slot.take().map(Err),
        }
    }
}

/// Read and parse `channels.json`.
fn read_channels(path: &Path) -> Result<Vec<ChannelEntry>, Error> {
    let bytes = fs::read(path).context(ReadSnafu { path: path.to_path_buf() })?;
    serde_json::from_slice(&bytes).context(ParseSnafu { path: path.to_path_buf() })
}

/// Read and parse the users map, returning an empty map when the file is
/// absent (the adapter then resolves names from message-embedded profiles).
fn read_users(path: &Path) -> Result<UserMap, Error> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserMap::default());
        }
        Err(error) => return Err(error).context(ReadSnafu { path: path.to_path_buf() }),
    };
    let entries: Vec<UserEntry> = serde_json::from_slice(&bytes).context(ParseSnafu { path: path.to_path_buf() })?;
    Ok(UserMap::from_entries(entries))
}
