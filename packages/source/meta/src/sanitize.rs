//! Ingestion-side body hygiene: one shared sanitizer every source adapter
//! applies before a body is hashed and embedded.
//!
//! Three concerns, one pipeline ([`sanitize`]):
//!
//! 1. **Terminal noise.** Raw ANSI/OSC escape sequences from captured tool
//!    output pollute embeddings (the live store matches `\x1b[1m` verbatim);
//!    [`strip_ansi`] removes them.
//! 2. **Secrets.** Live credentials have been found verbatim in indexed agent
//!    transcripts (a Linear API key in 200+ chunks, GitHub tokens in tool
//!    output). [`redact_secrets`] replaces every match of a known credential
//!    shape with `[redacted:<kind>]`. The table is conservative (prefixed,
//!    high-precision patterns only) and composable: adding a kind is one row.
//! 3. **Blobs.** Base64/hex payloads carry no retrievable meaning and dominate
//!    embeddings; [`collapse_blobs`] folds any whitespace-free run longer than
//!    [`BLOB_TOKEN_CHARS`] into a short `[blob NNN chars]` marker.
//!
//! [`sanitize_tool_result`] additionally caps one tool-result section at about
//! [`TOOL_RESULT_CAP_CHARS`] characters (head + tail around a
//! `[truncated NNN chars]` marker) so a multi-hundred-kilobyte CI log cannot
//! dominate the document it is folded into.
//!
//! Because `content_hash` is [`hash_body`](crate::hash_body) over the embedded
//! bytes, sanitizing **before** hashing means a re-sync sees previously
//! ingested raw bodies as changed and re-uploads the clean form — the
//! already-stored chunks get replaced on the next full pass.

use lazy_regex::regex::NoExpand;
use lazy_regex::{Regex, regex};

/// Blob threshold: longer whitespace-free tokens collapse to a marker.
///
/// Prose and code lines stay under this many characters per token, while
/// base64/hex payloads, minified bundles, and data URIs exceed it and become
/// `[blob NNN chars]`.
pub const BLOB_TOKEN_CHARS: usize = 120;

/// Maximum characters one tool-result section keeps before
/// [`sanitize_tool_result`] truncates it to head + tail around a marker.
pub const TOOL_RESULT_CAP_CHARS: usize = 4000;

/// Characters kept from the start of an over-cap tool-result section.
pub const CAP_HEAD_CHARS: usize = 3000;
/// Characters kept from the end of an over-cap tool-result section.
pub const CAP_TAIL_CHARS: usize = 1000;

/// One row of the redaction table: a credential kind and the pattern that
/// recognizes it. Matches become `[redacted:<kind>]`.
struct SecretPattern {
    /// Short label written into the redaction marker.
    kind: &'static str,
    /// Pattern matching the credential, compiled once.
    pattern: &'static Regex,
}

/// The redaction table. Ordered: multi-line blocks first, then specific token
/// prefixes (so `Authorization: Bearer ghp_...` redacts as the precise token
/// kind), then the generic `Authorization` header catch-all. Extending coverage
/// is adding one row.
fn secret_patterns() -> [SecretPattern; 10] {
    [
        // PEM-style private key blocks. If the END marker is missing (a
        // truncated capture), redact through to the end of the text: leaking
        // nothing beats preserving half a key.
        SecretPattern {
            kind: "private_key",
            pattern: regex!(
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|\z)"
            ),
        },
        SecretPattern {
            kind: "linear_api_key",
            pattern: regex!(r"\blin_api_[A-Za-z0-9]+"),
        },
        // Classic GitHub tokens: personal (ghp), OAuth (gho), user-to-server
        // (ghu), server-to-server (ghs), refresh (ghr); 36+ alphanumerics.
        SecretPattern {
            kind: "github_token",
            pattern: regex!(r"\bgh[pousr]_[A-Za-z0-9]{36,}"),
        },
        SecretPattern {
            kind: "github_pat",
            pattern: regex!(r"\bgithub_pat_[A-Za-z0-9_]+"),
        },
        // OpenAI/Anthropic-style keys (covers `sk-ant-...`). `\b` keeps the
        // `sk-` inside hyphenated words (`flask-...`) from matching.
        SecretPattern {
            kind: "sk_api_key",
            pattern: regex!(r"\bsk-[A-Za-z0-9_-]{20,}"),
        },
        SecretPattern {
            kind: "slack_token",
            pattern: regex!(r"\bxox[abprs]-[A-Za-z0-9-]+"),
        },
        SecretPattern {
            kind: "aws_access_key_id",
            pattern: regex!(r"\bAKIA[0-9A-Z]{16}\b"),
        },
        // Signed JWTs: three base64url segments, first long enough to be a
        // real header (short demo strings stay).
        SecretPattern {
            kind: "jwt",
            pattern: regex!(r"\beyJ[A-Za-z0-9_-]{40,}\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"),
        },
        // Generic HTTP auth headers, after the specific token kinds so a
        // recognizable token wins the more precise label.
        SecretPattern {
            kind: "authorization_header",
            pattern: regex!(r"(?i)\bauthorization:\s*(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]+"),
        },
        // Linear's API uses the bare key as the Authorization value (no
        // scheme); after `lin_api_` redaction this also catches other bare
        // opaque values long enough to be credentials.
        SecretPattern {
            kind: "authorization_header",
            pattern: regex!(r"(?i)\bauthorization:\s*[A-Za-z0-9._~+/=-]{20,}"),
        },
    ]
}

/// ANSI escape sequences: OSC (`ESC ]` ... `BEL`/`ST`), CSI
/// (`ESC [` params final-byte), and the remaining two-byte Fe escapes. OSC and
/// CSI come first in the alternation because `]` and `[` are themselves in the
/// Fe range.
fn ansi_pattern() -> &'static Regex {
    regex!(r"\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\)?|\[[0-?]*[ -/]*[@-~]|[@-Z\\-_])")
}

/// The full hygiene pipeline for an embeddable body: strip ANSI/OSC escapes,
/// redact credential shapes, collapse blob tokens. Idempotent: running it on
/// already-sanitized text changes nothing.
#[must_use]
pub fn sanitize(text: &str) -> String {
    let stripped = strip_ansi(text);
    let redacted = redact_secrets(&stripped);
    collapse_blobs(&redacted)
}

/// [`sanitize`] plus the per-section size cap for tool results.
///
/// An over-cap section keeps its head and tail around a
/// `[truncated NNN chars]` marker, so a giant CI log stops dominating the
/// document (and the embedding) it is folded into.
#[must_use]
pub fn sanitize_tool_result(text: &str) -> String {
    cap_section(&sanitize(text))
}

/// Remove ANSI/OSC escape sequences (colors, cursor movement, titles).
#[must_use]
pub fn strip_ansi(text: &str) -> String {
    ansi_pattern().replace_all(text, "").into_owned()
}

/// Replace every match of the redaction table with `[redacted:<kind>]`.
#[must_use]
pub fn redact_secrets(text: &str) -> String {
    let mut current = text.to_owned();
    for entry in secret_patterns() {
        if entry.pattern.is_match(&current) {
            let marker = format!("[redacted:{}]", entry.kind);
            current = entry
                .pattern
                .replace_all(&current, NoExpand(&marker))
                .into_owned();
        }
    }
    current
}

/// Collapse every whitespace-free run longer than [`BLOB_TOKEN_CHARS`]
/// characters to `[blob NNN chars]`.
///
/// Whitespace (and therefore line structure) is preserved exactly; only the
/// over-long tokens are replaced.
#[must_use]
pub fn collapse_blobs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token_start: Option<usize> = None;
    let mut token_chars = 0usize;
    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                push_token(&mut out, &text[start..index], token_chars);
            }
            token_chars = 0;
            out.push(ch);
        } else {
            if token_start.is_none() {
                token_start = Some(index);
            }
            token_chars += 1;
        }
    }
    if let Some(start) = token_start {
        push_token(&mut out, &text[start..], token_chars);
    }
    out
}

/// Append one whitespace-free token, collapsing it to a marker when over the
/// blob threshold.
fn push_token(out: &mut String, token: &str, chars: usize) {
    if chars > BLOB_TOKEN_CHARS {
        out.push_str("[blob ");
        out.push_str(&chars.to_string());
        out.push_str(" chars]");
    } else {
        out.push_str(token);
    }
}

/// Cap one section at [`TOOL_RESULT_CAP_CHARS`] characters: keep the first
/// [`CAP_HEAD_CHARS`] and last [`CAP_TAIL_CHARS`] around a
/// `[truncated NNN chars]` marker. Under-cap text passes through unchanged.
#[must_use]
pub fn cap_section(text: &str) -> String {
    let total = text.chars().count();
    if total <= TOOL_RESULT_CAP_CHARS {
        return text.to_owned();
    }
    let head_end = byte_index_at(text, CAP_HEAD_CHARS);
    let tail_start = byte_index_at(text, total - CAP_TAIL_CHARS);
    let omitted = total - CAP_HEAD_CHARS - CAP_TAIL_CHARS;
    let head = text.get(..head_end).unwrap_or_default();
    let tail = text.get(tail_start..).unwrap_or_default();
    format!("{head}\n[truncated {omitted} chars]\n{tail}")
}

/// Byte offset of the `chars`-th character, or the text's length when shorter.
fn byte_index_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    // NOTE: no test below embeds anything resembling a real credential; every
    // fixture token is constructed at test time from repeated filler.

    use super::{
        BLOB_TOKEN_CHARS, TOOL_RESULT_CAP_CHARS, cap_section, collapse_blobs, redact_secrets,
        sanitize, sanitize_tool_result, strip_ansi,
    };

    #[test]
    fn ansi_csi_and_osc_are_stripped() {
        let input = "\u{1b}[1mbold\u{1b}[0m \u{1b}]0;window title\u{7}plain \u{1b}[38;5;196mred";
        assert_eq!(strip_ansi(input), "bold plain red");
    }

    #[test]
    fn osc_with_st_terminator_is_stripped() {
        let input = "a\u{1b}]8;;https://example.com\u{1b}\\link\u{1b}]8;;\u{1b}\\b";
        assert_eq!(strip_ansi(input), "alinkb");
    }

    #[test]
    fn plain_prose_passes_through_unchanged() {
        let input = "fix the bug in transcript.rs\n\n[tool_use Bash] {\"command\":\"ls\"}";
        assert_eq!(sanitize(input), input);
    }

    #[test]
    fn linear_api_key_is_redacted() {
        let key = format!("lin_api_{}", "a1B".repeat(13));
        let input = format!("API_KEY = \"{key}\"");
        let output = redact_secrets(&input);
        assert!(!output.contains(&key), "{output}");
        assert_eq!(output, "API_KEY = \"[redacted:linear_api_key]\"");
    }

    #[test]
    fn github_token_family_is_redacted() {
        for prefix in ["ghp", "gho", "ghu", "ghs", "ghr"] {
            let token = format!("{prefix}_{}", "Ab1".repeat(12));
            let output = redact_secrets(&format!("token: {token}"));
            assert_eq!(output, "token: [redacted:github_token]", "prefix {prefix}");
        }
        let pat = format!("github_pat_{}_{}", "X9".repeat(11), "y8".repeat(29));
        assert_eq!(
            redact_secrets(&format!("use {pat} here")),
            "use [redacted:github_pat] here"
        );
    }

    #[test]
    fn sk_key_is_redacted_but_hyphenated_words_are_not() {
        let key = format!("sk-ant-{}", "w0".repeat(20));
        assert_eq!(
            redact_secrets(&format!("key={key}")),
            "key=[redacted:sk_api_key]"
        );
        let prose = "install flask-sqlalchemy-some-very-long-extension";
        assert_eq!(redact_secrets(prose), prose);
    }

    #[test]
    fn slack_aws_and_jwt_are_redacted() {
        let slack = format!("xoxb-{}-{}", "1".repeat(12), "a".repeat(24));
        assert_eq!(
            redact_secrets(&format!("S={slack}")),
            "S=[redacted:slack_token]"
        );

        let aws = format!("AKIA{}", "J5".repeat(8));
        assert_eq!(
            redact_secrets(&format!("id {aws} end")),
            "id [redacted:aws_access_key_id] end"
        );

        let jwt = format!("eyJ{}.{}.{}", "h".repeat(41), "p".repeat(30), "s".repeat(43));
        assert_eq!(redact_secrets(&format!("t={jwt}")), "t=[redacted:jwt]");
    }

    #[test]
    fn private_key_block_is_redacted_even_when_truncated() {
        let block = format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
            "MIIE".repeat(16)
        );
        let output = redact_secrets(&format!("before\n{block}\nafter"));
        assert_eq!(output, "before\n[redacted:private_key]\nafter");

        let truncated = format!("before\n-----BEGIN PRIVATE KEY-----\n{}", "MIIE".repeat(16));
        assert_eq!(
            redact_secrets(&truncated),
            "before\n[redacted:private_key]"
        );
    }

    #[test]
    fn authorization_headers_are_redacted() {
        let bearer = format!("Authorization: Bearer {}", "tok9".repeat(10));
        assert_eq!(
            redact_secrets(&bearer),
            "[redacted:authorization_header]"
        );
        let basic = format!("authorization: basic {}=", "dXNlcjpw".repeat(4));
        assert_eq!(redact_secrets(&basic), "[redacted:authorization_header]");
        // Linear style: bare key as the header value.
        let bare = format!("-H \"Authorization: lin_api_{}\"", "Qq2".repeat(13));
        assert_eq!(
            redact_secrets(&bare),
            "-H \"Authorization: [redacted:linear_api_key]\""
        );
    }

    #[test]
    fn long_blob_tokens_collapse_and_short_ones_stay() {
        let blob = "QUJD/0+=".repeat(40); // 320 whitespace-free chars
        let input = format!("data {blob} end");
        assert_eq!(collapse_blobs(&input), "data [blob 320 chars] end");

        let under = "x".repeat(BLOB_TOKEN_CHARS);
        let kept = format!("ok {under} ok");
        assert_eq!(collapse_blobs(&kept), kept);
    }

    #[test]
    fn blob_collapse_preserves_line_structure() {
        let blob = "ff00".repeat(50);
        let input = format!("line one\n{blob}\nline three");
        assert_eq!(
            collapse_blobs(&input),
            "line one\n[blob 200 chars]\nline three"
        );
    }

    #[test]
    fn redaction_wins_over_blob_collapse_for_long_keys() {
        // A 300+ char fake credential must surface as a redaction marker, not
        // be hidden as a generic blob.
        let key = format!("lin_api_{}", "z".repeat(300));
        let output = sanitize(&format!("k={key}"));
        assert_eq!(output, "k=[redacted:linear_api_key]");
    }

    #[test]
    fn over_cap_section_keeps_head_and_tail_with_marker() {
        let head_marker = "START-OF-LOG ";
        let tail_marker = " END-OF-LOG";
        let filler = "x".repeat(10_000);
        let input = format!("{head_marker}{filler}{tail_marker}");
        let total = input.chars().count();

        let output = cap_section(&input);
        assert!(output.starts_with(head_marker), "head preserved");
        assert!(output.ends_with(tail_marker), "tail preserved");
        let omitted = total - 4000;
        assert!(
            output.contains(&format!("[truncated {omitted} chars]")),
            "{output}"
        );
        assert!(output.chars().count() < total, "shorter than the input");
    }

    #[test]
    fn under_cap_section_is_untouched() {
        let input = "y".repeat(TOOL_RESULT_CAP_CHARS);
        assert_eq!(cap_section(&input), input);
    }

    #[test]
    fn sanitize_tool_result_composes_all_stages() {
        let key = format!("ghp_{}", "Cc7".repeat(12));
        let blob = "abc123+/".repeat(40);
        let noise = "\u{1b}[31mError:\u{1b}[0m build failed\n".repeat(400);
        let input = format!("{noise}token {key}\npayload {blob}\n");

        let output = sanitize_tool_result(&input);
        assert!(!output.contains('\u{1b}'), "ANSI stripped");
        assert!(!output.contains(&key), "secret gone");
        assert!(output.contains("[truncated"), "capped: {} chars", output.len());
        assert!(output.contains("[redacted:github_token]"), "{output}");
        assert!(output.contains("[blob 320 chars]"), "{output}");
        assert!(
            output.chars().count() <= TOOL_RESULT_CAP_CHARS + 64,
            "respects the cap"
        );
    }

    #[test]
    fn sanitize_is_idempotent() {
        let key = format!("xoxb-{}", "3".repeat(30));
        let input = format!("\u{1b}[1mkey\u{1b}[0m {key} blob {}", "Zz".repeat(100));
        let once = sanitize(&input);
        assert_eq!(sanitize(&once), once);
    }
}
