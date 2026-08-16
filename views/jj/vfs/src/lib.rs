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

//! Serve a Jujutsu commit tree as a read-only filesystem.
//!
//! The crate is split into a transport-agnostic core ([`snapshot`]) and one
//! adapter per kernel interface. The core answers the five questions any
//! read-only filesystem has to answer (lookup, getattr, readdir, read,
//! readlink) against an immutable [`jj_lib::merged_tree::MergedTree`], and
//! knows nothing about FUSE or NFS.
//!
//! v0 is read-only and therefore records nothing, but the seam a write path
//! would record through is already here: see [`journal`], which also carries
//! the reasoning for why a virtual filesystem can make jj's operation log
//! complete rather than merely cheap.
//!
//! Two adapters exist because no single one covers both platforms. FUSE is the
//! fast path but Linux-only for us, since macOS FUSE means macFUSE, a
//! third-party kernel extension requiring a reduced security posture. NFSv3
//! over loopback works everywhere, including unprivileged on macOS, because
//! every supported OS ships an NFS client in its own kernel. This is the same
//! split Meta's EdenFS uses.

pub mod journal;
pub mod overlay;
pub mod snapshot;
pub mod stats;

mod sys;

#[cfg(all(feature = "fuse", target_os = "linux"))]
pub mod fuse;
#[cfg(feature = "nfs")]
pub mod nfs;

pub use overlay::Overlay;
pub use overlay::OverlayTree;
pub use overlay::WHITEOUT_NAME;
pub use snapshot::Attributes;
pub use snapshot::DEFAULT_CONTENT_CACHE_BYTES;
pub use snapshot::EntryKind;
pub use snapshot::ROOT_INODE;
pub use snapshot::SnapshotError;
pub use snapshot::TreeEntry;
pub use snapshot::TreeSnapshot;
pub use snapshot::default_materialize_options;
pub use stats::Op;
pub use stats::Stats;
