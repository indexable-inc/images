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

//! The last survey of each view, persisted where a cheap reader can find it.
//!
//! Positioning a view against its published repository costs a derive over
//! the whole prefix history -- seconds on a large repository -- so nothing
//! rendered per prompt can compute it. The commands that survey have already
//! paid that cost; writing what they saw costs one small file, and `jj views
//! prompt` then answers from the file alone. The freshness contract is a git
//! remote-tracking ref's: true as of the last survey, not of now.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use super::Survey;
use crate::command_error::CommandError;
use crate::command_error::user_error_with_message;

/// Directory under `.jj/repo/` the records live in, one JSON file per view.
const DIR: &str = "views";

/// What one survey saw, flattened to what a later reader uses.
///
/// The two commit ids name what the counts compared, so a reader can tell a
/// record about today's bookmark from one the bookmark has since moved away
/// from.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SurveyRecord {
    /// Bookmark the survey anchored on, and the commit it pointed at.
    pub bookmark: String,
    pub anchor: String,
    /// What the published repository's branch pointed at.
    pub upstream: String,
    /// Published commits that had not arrived here: how far behind this
    /// repository was.
    pub incoming: usize,
    /// View commits here the published repository did not have.
    pub ahead: usize,
}

/// Writes `survey` as the record for its view.
///
/// Failing to write is a broken `.jj/repo`, reported rather than swallowed:
/// the command that surveyed has already written operations to the same
/// directory, so an error here means the store is in trouble, not the record.
pub fn write(
    repo_path: &Path,
    bookmark: &str,
    anchor: &gix::ObjectId,
    survey: &Survey,
) -> Result<(), CommandError> {
    let record = SurveyRecord {
        bookmark: bookmark.to_owned(),
        anchor: anchor.to_string(),
        upstream: survey.upstream.to_string(),
        incoming: survey.incoming.len(),
        ahead: survey.ahead,
    };
    let name = &survey.view.name;
    let dir = repo_path.join(DIR);
    fs::create_dir_all(&dir).map_err(|err| could_not(name, err))?;
    // Through a temporary file so a concurrent `jj views prompt` reads the
    // old record or the new one, never a half-written one.
    let mut file = tempfile::NamedTempFile::new_in(&dir).map_err(|err| could_not(name, err))?;
    serde_json::to_writer_pretty(&mut file, &record).map_err(|err| could_not(name, err))?;
    file.write_all(b"\n").map_err(|err| could_not(name, err))?;
    file.persist(path(repo_path, name))
        .map_err(|err| could_not(name, err.error))?;
    Ok(())
}

fn could_not(name: &str, err: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> CommandError {
    user_error_with_message(format!("Could not record the {name} view's survey"), err)
}

/// Reads the record the last survey of `name` left, if any survives.
///
/// A record that fails to parse reads as no record: the only way to produce
/// one is a jj whose record format has since changed, and a stale format
/// carries the same information as no survey at all.
pub fn read(repo_path: &Path, name: &str) -> Option<SurveyRecord> {
    let raw = fs::read_to_string(path(repo_path, name)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn path(repo_path: &Path, name: &str) -> PathBuf {
    repo_path.join(DIR).join(format!("{name}.json"))
}
