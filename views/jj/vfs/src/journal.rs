// Copyright 2026 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The seam a write path records through.
//!
//! Nothing writes to this yet, because a read-only mount has no changes to
//! record. It exists now because retrofitting it is much harder than designing
//! for it: the shape below is what decides whether a future write path can
//! answer "what did this tree look like at 14:03" cheaply, and that shape has
//! to be chosen before there are two transports each doing their own thing.
//!
//! # Why a journal at all, when jj already has an operation log
//!
//! jj's operation log records every change to refs, which is genuinely good,
//! but it learns about the working copy by polling: each command walks the tree
//! and diffs it. So everything between two commands collapses into one net
//! change. Edit a file five times between two jj commands and jj sees one edit;
//! the intermediate states never existed as far as jj is concerned.
//!
//! A virtual filesystem is not a poller, it *is* the write path. Every write,
//! create, rename, unlink, truncate and mode change arrives here as a request.
//! The complete ordered sequence is therefore available for free, and the only
//! way to not have it is to throw it away.
//!
//! # The design this is shaped for
//!
//! Google's CitC, described at JJ Con 2025, splits a workspace into a baseline
//! (the state at a submitted commit) and an overlay (the files the user has
//! changed). That split is what makes snapshotting cheap, and it is also the
//! thing to grow into rather than around.
//!
//! The observation worth writing down: **the overlay is the journal's current
//! state.** So build the overlay as a replay of an append-only log rather than
//! as a mutable tree, and "reconstruct the working copy at any instant" falls
//! out of the same structure that made snapshotting cheap, instead of needing a
//! second history mechanism beside it. That is the strong version of the
//! design, where the journal is the source of truth and the visible tree is a
//! materialized view of it. The cheap version of that strong design is to keep
//! the overlay's authority in the log from the start, even while materializing
//! eagerly, so the eager materialization is an optimization that can be relaxed
//! rather than a decision that has to be undone.
//!
//! The weaker alternative, a journal as an audit trail beside an authoritative
//! mutable tree, is easier to build and cannot answer the question the log
//! exists for without replaying against a tree that has already moved on.
//!
//! # What is deliberately not decided here
//!
//! Retention and exclusion. Recording every write captures editor swap files,
//! build output, and a secret typed into a file and deleted a minute later.
//! Excluding by gitignore is not sufficient, because the point is to record
//! what git would not see.
//!
//! Coalescing. It is the knob that destroys the property this exists for, so
//! any coalescing needs a stated rule and a reason rather than a heuristic.

use std::time::SystemTime;

use jj_lib::backend::FileId;
use jj_lib::repo_path::RepoPathBuf;

/// What happened to a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Content was written. The bytes are referenced, not inlined; see
    /// [`ContentRef`].
    Write {
        /// The content after the write.
        content: ContentRef,
    },
    /// A path came into existence.
    Create,
    /// A path stopped existing.
    Remove,
    /// A path moved. The entry's own path is the destination.
    Rename {
        /// Where it came from.
        from: RepoPathBuf,
    },
    /// A file was shortened or extended.
    Truncate {
        /// The length afterwards.
        length: u64,
    },
    /// The executable bit changed.
    SetExecutable {
        /// Its value afterwards.
        executable: bool,
    },
}

/// How a journal entry refers to content.
///
/// By reference and content-addressed, so that writing the same bytes twice
/// costs one copy rather than two, and so that a journal of a `cargo build` is
/// a list of ids rather than a second copy of `target/`. Small content can be
/// inlined because storing an id for nine bytes costs more than the nine bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContentRef {
    /// Content already in the jj store, named by its id.
    Stored(FileId),
    /// Content small enough that a reference would be larger than the bytes.
    Inline(Vec<u8>),
}

/// Who made a change, where the transport can say.
///
/// This is the one real asymmetry between the two transports and it does not
/// have a workaround. A FUSE request carries the calling process's pid and uid,
/// so on Linux an entry can say which process wrote which file in what order.
/// An NFSv3 request carries no such field, so on macOS the journal records what
/// changed and when but not who. EdenFS hit the same wall and fell back to
/// sampling `lsof` and `fs_usage`, which is not attribution.
///
/// The field is [`Option`] rather than a filled-in guess precisely so that the
/// absence is visible in the data instead of being papered over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Actor {
    /// Calling process id.
    pub pid: u32,
    /// Calling user id.
    pub uid: u32,
}

/// One recorded change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// Monotonic position in the log. The ordering is the point of the journal,
    /// so it is recorded rather than inferred from timestamps, which can tie.
    pub sequence: u64,
    /// When it happened.
    pub timestamp: SystemTime,
    /// The path affected.
    pub path: RepoPathBuf,
    /// What happened.
    pub operation: Operation,
    /// Who did it, when the transport can say. See [`Actor`].
    pub actor: Option<Actor>,
}

/// Why an entry could not be recorded.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// The journal could not be written.
    #[error("Failed to record a journal entry: {source}")]
    Write {
        /// The underlying reason.
        source: std::io::Error,
    },
}

/// Somewhere to record changes.
///
/// Two constraints on any implementation, both of which are easier to honor by
/// construction than to bolt on.
///
/// It must write to real storage, never through the mount it is recording. Over
/// NFS neither the macOS nor the Linux client sends COMMIT on `fsync`, so a
/// journal written through the mount can lose its tail while looking complete,
/// and a journal that can lose its tail is worse than no journal because it
/// looks whole. The NixOS module refuses a `journalDirectory` inside the mount
/// point at eval time for this reason.
///
/// And the ordering between "the write is visible to the client" and "the write
/// is in the journal" has to be stated by the implementation. Recording first
/// is the honest default: a client that sees a write which is not in the log
/// breaks the log's only promise, whereas a log entry for a write the client
/// never observed is merely a redundant record.
pub trait Journal: Send + Sync + std::fmt::Debug {
    /// Records one change.
    fn record(&self, entry: Entry) -> Result<(), JournalError>;
}

/// A journal that records nothing.
///
/// What a read-only mount uses, because there is nothing to record. It exists
/// so that the write path, when there is one, has no "if journaling is
/// configured" branch: there is always a journal, and this is the one that does
/// nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullJournal;

impl Journal for NullJournal {
    fn record(&self, _entry: Entry) -> Result<(), JournalError> {
        Ok(())
    }
}
