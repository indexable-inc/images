//! Human-readable session id generation.
//!
//! Ids look like `amber-otter-3f`: an adjective, a noun, and a short suffix from
//! the daemon PID so two sessions started in the same instant still differ. No
//! crypto-grade randomness is needed; this is a friendly label, not a secret.

use std::time::{SystemTime, UNIX_EPOCH};

const ADJECTIVES: &[&str] = &[
    "amber", "brave", "calm", "clever", "cosmic", "dapper", "eager", "fuzzy", "gentle", "glossy",
    "humble", "jolly", "keen", "lively", "mellow", "nimble", "polished", "quiet", "rapid", "sly",
    "snug", "spry", "sunny", "swift", "tidy", "vivid", "witty", "zesty",
];

const NOUNS: &[&str] = &[
    "otter", "falcon", "maple", "comet", "harbor", "lynx", "pebble", "quartz", "raven", "willow",
    "badger", "cedar", "dolphin", "ember", "fjord", "glade", "heron", "ibex", "juniper", "koala",
    "marlin", "nebula", "orchid", "puffin", "robin", "spruce", "thistle", "walrus",
];

/// Generate a fresh, human-readable session id.
#[must_use]
pub fn generate() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let pid = std::process::id();

    let adjective = ADJECTIVES[(nanos as usize) % ADJECTIVES.len()];
    let noun = NOUNS[(nanos as usize / ADJECTIVES.len() + pid as usize) % NOUNS.len()];
    let suffix = pid % 256;

    format!("{adjective}-{noun}-{suffix:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_three_dashed_segments() {
        let id = generate();
        assert_eq!(id.split('-').count(), 3, "unexpected id shape: {id}");
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}
