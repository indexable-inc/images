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

//! Agent provenance: resolve which AI agent (if any) is driving this jj
//! invocation so that every operation can be stamped with agent identity.
//!
//! Resolution is strictly best-effort. A missing, partial, or malformed
//! source means "no agent context"; it must never fail or slow down a jj
//! command for a human user.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;

/// Identity of the agent driving this jj invocation.
///
/// `session` is the field that makes a context "present": without a session
/// id there is no context at all. Every other field is optional.
#[derive(Clone, Debug)]
pub struct AgentContext {
    /// Kind of agent, e.g. "claude-code". Defaults to "unknown" when the
    /// context came from explicit `JJ_AGENT_*` variables without a kind, and
    /// to "claude-code" when it came from the Claude Code fallback file.
    pub kind: String,
    /// Opaque session identifier.
    pub session: String,
    /// Path to the agent's transcript file, if any.
    pub transcript: Option<String>,
    /// Model identifier, e.g. "claude-fable-5".
    pub model: Option<String>,
    /// Reasoning effort setting.
    pub effort: Option<String>,
    /// Agent software version.
    pub version: Option<String>,
    /// Parent session/agent identifier for nested agents.
    pub parent: Option<String>,
}

/// On-disk schema of `~/.claude/session-by-pid/<pid>.json`, written by the
/// Claude Code harness. All fields are optional strings.
#[derive(Debug, Deserialize)]
struct SessionByPidEntry {
    session_id: Option<String>,
    transcript: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    version: Option<String>,
    parent: Option<String>,
    kind: Option<String>,
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

/// The wire form of a transcript path: its file name, never its directories.
///
/// A transcript's absolute path names the directory the agent session was
/// opened in, and a directory name can itself be the sensitive fact --- it
/// discloses that a given project exists and that sessions in it drove this
/// repository. Operation attributes are not local: they are replicated to
/// whatever server holds the operation log and rendered to everyone who can
/// read it, so the path would travel to a strictly wider audience than the
/// transcript it points at.
///
/// The file name is the part that identifies the transcript --- it is a
/// unique id in every producer we have --- and the directories are the only
/// part that leaks, so keeping the name loses nothing an owner needs.
///
/// This lives at the boundary that writes the attribute rather than in any
/// one producer, so the guarantee holds for every source of a transcript
/// value instead of for whichever producer remembered to redact.
///
/// Postcondition: the returned string contains no path separator.
fn wire_transcript(value: &str) -> &str {
    // Trailing separators first, so a value that names a directory cannot
    // return an empty last component and fall back to the full path.
    value
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
}

impl AgentContext {
    /// Resolves the agent context from the environment.
    ///
    /// Primary source: `JJ_AGENT_SESSION` (presence marker) plus the other
    /// `JJ_AGENT_*` variables. Fallback: a Claude Code session inferred from
    /// `CLAUDE_CODE_MESSAGING_SOCKET` (`/tmp/cc-socks/<pid>.sock`) and the
    /// pid-to-session map file `<map dir>/<pid>.json`, where the map dir is
    /// `JJ_AGENT_MAP_DIR` or `~/.claude/session-by-pid`.
    pub fn resolve_from_env() -> Option<Self> {
        if let Some(session) = env_nonempty("JJ_AGENT_SESSION") {
            return Some(Self {
                kind: env_nonempty("JJ_AGENT_KIND").unwrap_or_else(|| "unknown".to_owned()),
                session,
                transcript: env_nonempty("JJ_AGENT_TRANSCRIPT"),
                model: env_nonempty("JJ_AGENT_MODEL"),
                effort: env_nonempty("JJ_AGENT_EFFORT"),
                version: env_nonempty("JJ_AGENT_VERSION"),
                parent: env_nonempty("JJ_AGENT_PARENT"),
            });
        }
        Self::resolve_claude_code_fallback()
    }

    fn resolve_claude_code_fallback() -> Option<Self> {
        let socket = env_nonempty("CLAUDE_CODE_MESSAGING_SOCKET")?;
        let pid = Path::new(&socket).file_stem()?.to_str()?;
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let map_dir = match env_nonempty("JJ_AGENT_MAP_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => etcetera::home_dir()
                .ok()?
                .join(".claude")
                .join("session-by-pid"),
        };
        let raw = fs::read_to_string(map_dir.join(format!("{pid}.json"))).ok()?;
        let entry: SessionByPidEntry = serde_json::from_str(&raw).ok()?;
        let session = nonempty(entry.session_id)?;
        Some(Self {
            kind: nonempty(entry.kind).unwrap_or_else(|| "claude-code".to_owned()),
            session,
            transcript: nonempty(entry.transcript),
            model: nonempty(entry.model),
            effort: nonempty(entry.effort),
            version: nonempty(entry.version),
            parent: nonempty(entry.parent),
        })
    }

    /// Operation-metadata attributes for this context. Frozen schema, shared
    /// with provenance consumers: `agent.kind`, `agent.session`,
    /// `agent.transcript`, `agent.model`, `agent.effort`, `agent.version`,
    /// `agent.parent`, and `agent.turn_offset` (byte length of the transcript
    /// file at operation time, via a single stat; the file is never read).
    ///
    /// `agent.transcript` is emitted as a file name only (see
    /// [`wire_transcript`]); `agent.turn_offset` is measured against the
    /// unredacted path, so redacting the attribute does not cost the offset.
    /// The two are deliberately read from different values: what identifies
    /// the transcript to a reader and what locates it on this machine are
    /// different questions, and only the first one belongs on the wire.
    pub fn to_transaction_attributes(&self) -> Vec<(String, String)> {
        let mut attributes = vec![
            ("agent.kind".to_owned(), self.kind.clone()),
            ("agent.session".to_owned(), self.session.clone()),
        ];
        if let Some(transcript) = &self.transcript {
            attributes.push((
                "agent.transcript".to_owned(),
                wire_transcript(transcript).to_owned(),
            ));
        }
        let optional = [
            ("agent.model", &self.model),
            ("agent.effort", &self.effort),
            ("agent.version", &self.version),
            ("agent.parent", &self.parent),
        ];
        for (key, value) in optional {
            if let Some(value) = value {
                attributes.push((key.to_owned(), value.clone()));
            }
        }
        if let Some(transcript) = &self.transcript {
            // O(1) stat only: transcripts can be hundreds of megabytes, and a
            // failed stat silently omits the offset rather than erroring.
            // Deliberately the unredacted `transcript`: a bare file name does
            // not resolve from jj's working directory, so statting the wire
            // form would silently drop this attribute for every caller.
            if let Ok(metadata) = fs::metadata(transcript) {
                attributes.push(("agent.turn_offset".to_owned(), metadata.len().to_string()));
            }
        }
        attributes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context_with_transcript(transcript: &str) -> AgentContext {
        AgentContext {
            kind: "claude-code".to_owned(),
            session: "sess".to_owned(),
            transcript: Some(transcript.to_owned()),
            model: None,
            effort: None,
            version: None,
            parent: None,
        }
    }

    fn transcript_attribute(context: &AgentContext) -> Option<String> {
        context
            .to_transaction_attributes()
            .into_iter()
            .find(|(key, _)| key == "agent.transcript")
            .map(|(_, value)| value)
    }

    #[test]
    fn wire_transcript_keeps_only_the_file_name() {
        assert_eq!(
            wire_transcript("/home/someone/.agent/projects/-a-project-dir/uuid.jsonl"),
            "uuid.jsonl"
        );
        assert_eq!(wire_transcript("/uuid.jsonl"), "uuid.jsonl");
        assert_eq!(wire_transcript("relative/dir/uuid.jsonl"), "uuid.jsonl");
    }

    #[test]
    fn wire_transcript_passes_a_bare_file_name_through_unchanged() {
        // Negative arm: with nothing to redact the value must survive
        // byte-for-byte, so a producer that already redacts is not punished
        // and the attribute stays a usable identifier.
        assert_eq!(wire_transcript("uuid.jsonl"), "uuid.jsonl");
        assert_eq!(wire_transcript("no-extension"), "no-extension");
        assert_eq!(wire_transcript(""), "");
    }

    #[test]
    fn wire_transcript_never_returns_a_separator() {
        // The postcondition is the actual guarantee, so it is asserted on the
        // shapes that would otherwise slip a directory through: a trailing
        // separator, and a value that is nothing but separators.
        for value in [
            "/a/b/",
            "/a/b//",
            "/",
            "//",
            "a\\b\\c.jsonl",
            "/mixed\\separators/x.jsonl",
        ] {
            let redacted = wire_transcript(value);
            assert!(
                !redacted.contains('/') && !redacted.contains('\\'),
                "{value:?} redacted to {redacted:?}, which still names a directory"
            );
        }
    }

    #[test]
    fn transaction_attributes_redact_an_absolute_transcript() {
        // The call-site arm: the helper being correct proves nothing about
        // what reaches the wire, so this asserts on the attribute list that
        // `start_repo_transaction` hands to `Transaction::set_attribute`.
        let context = context_with_transcript("/home/someone/projects/secret-project/uuid.jsonl");
        assert_eq!(transcript_attribute(&context).as_deref(), Some("uuid.jsonl"));
        for (key, value) in context.to_transaction_attributes() {
            assert!(
                !value.contains('/'),
                "attribute {key} carries a path: {value:?}"
            );
        }
    }

    #[test]
    fn transaction_attributes_pass_a_bare_file_name_through_unchanged() {
        let context = context_with_transcript("uuid.jsonl");
        assert_eq!(transcript_attribute(&context).as_deref(), Some("uuid.jsonl"));
    }

    #[test]
    fn turn_offset_survives_redaction_of_an_absolute_transcript() {
        // Redaction and the offset read the same field for different
        // purposes; this pins that fixing the leak did not silently cost the
        // offset, which is omitted rather than reported when it cannot be
        // taken.
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("uuid.jsonl");
        std::fs::write(&transcript, "twelve bytes").unwrap();

        let context = context_with_transcript(transcript.to_str().unwrap());
        let attributes = context.to_transaction_attributes();

        assert_eq!(transcript_attribute(&context).as_deref(), Some("uuid.jsonl"));
        assert_eq!(
            attributes
                .iter()
                .find(|(key, _)| key == "agent.turn_offset")
                .map(|(_, value)| value.as_str()),
            Some("12")
        );
    }
}
