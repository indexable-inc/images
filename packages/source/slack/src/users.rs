//! User-id resolution.
//!
//! Mentions and authors are stored as ids (`U0...`); we render and record the
//! human display name. Resolution follows a fixed fallback chain so a mention
//! is never dropped: the export-wide users map, then the profile embedded on
//! the message, then the raw id.

use std::collections::HashMap;

use crate::model::{UserEntry, UserProfile};

/// An index from user id to resolved display name and bot flag, built once from
/// `users.json`.
#[derive(Debug, Clone, Default)]
pub struct UserMap {
    by_id: HashMap<String, ResolvedUser>,
}

/// A user's resolved presentation: the name to show and whether it is a bot.
#[derive(Debug, Clone)]
pub struct ResolvedUser {
    /// The display name to render (never empty).
    pub name: String,
    /// Whether this user is a bot/integration account.
    pub is_bot: bool,
}

impl UserMap {
    /// Build a map from the parsed `users.json` entries.
    #[must_use]
    pub fn from_entries(entries: Vec<UserEntry>) -> Self {
        let mut by_id = HashMap::with_capacity(entries.len());
        for entry in entries {
            let name = entry
                .profile
                .best_name()
                .map_or_else(|| fallback_name(&entry.name, &entry.id).to_owned(), str::to_owned);
            by_id.insert(
                entry.id,
                ResolvedUser {
                    name,
                    is_bot: entry.is_bot,
                },
            );
        }
        Self { by_id }
    }

    /// Resolve a mention or author id to a display name, falling back to the
    /// message-embedded profile and finally the raw id. Never returns the empty
    /// string and never drops the id.
    #[must_use]
    pub fn resolve<'a>(&'a self, id: &'a str, message_profile: Option<&'a UserProfile>) -> &'a str {
        if let Some(user) = self.by_id.get(id) {
            return &user.name;
        }
        if let Some(name) = message_profile.and_then(UserProfile::best_name) {
            return name;
        }
        id
    }

    /// Whether the given id is a known bot account in the users map.
    #[must_use]
    pub fn is_bot(&self, id: &str) -> bool {
        self.by_id.get(id).is_some_and(|user| user.is_bot)
    }
}

/// The name to use when a profile has no usable display or real name: the
/// handle if present, otherwise the raw id (never empty).
fn fallback_name<'a>(handle: &'a str, id: &'a str) -> &'a str {
    let handle = handle.trim();
    if handle.is_empty() { id } else { handle }
}
