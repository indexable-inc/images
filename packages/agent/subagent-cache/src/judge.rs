//! Stage 2: the precision gate. A single Haiku-class Messages API call decides
//! which recalled candidates genuinely answer the new question.
//!
//! This is a one-shot structured yes/no, so it is a thin `reqwest` call to the
//! Anthropic Messages API rather than the `claude` agent CLI (which exists for
//! multi-turn turns, not a stateless classification) or a vendored SDK (a large
//! surface for one endpoint). The key is read from the process environment and
//! never placed on argv.
//!
//! The gate is governed by the correctness asymmetry: a false miss only costs a
//! redundant cold run, a false hit serves a wrong answer. So the prompt is
//! biased to exclude under doubt, and any decode/parse failure is fail-closed
//! (treated as "no candidate matched").

use serde::Deserialize;

use crate::error::{JudgeDecodeSnafu, JudgeSendSnafu, JudgeStatusSnafu, Result};
use crate::store::RecallRow;

const SYSTEM: &str = "You are a strict cache-hit judge. You are given a NEW question and \
a numbered list of CACHED investigations (each: the question it answered and the files it \
read). Decide which cached investigations genuinely answer the NEW question with the same \
intent and scope. Be conservative: if a cached entry is merely on the same topic but a \
different question, depth, or scope, EXCLUDE it. A wrong inclusion serves a stale or \
off-target answer, which is far worse than excluding a usable one. Reply with ONLY a \
comma-separated list of the matching numbers (e.g. `1,3`), or `NONE` if none match.";

/// Path cap so a wide investigation does not blow up the judge prompt.
const MAX_DEP_PATHS: usize = 25;

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: String,
}

/// `max_tokens` for the judge reply: it only ever emits a short index list.
const JUDGE_MAX_TOKENS: u32 = 64;

/// The Anthropic Messages endpoint the judge calls, bundled so the call site
/// stays within the 3-argument limit.
#[derive(Debug, Clone, Copy)]
pub struct JudgeApi<'a> {
    pub http: &'a reqwest::Client,
    pub api_base: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
}

/// Returns the indices (into `candidates`) the judge accepts, preserving the
/// best-first recall order. An empty result means run cold.
///
/// # Errors
/// Errors if the judge request fails to send, returns a non-success HTTP
/// status, or the response body cannot be decoded.
pub async fn judge(
    api: &JudgeApi<'_>,
    new_prompt: &str,
    candidates: &[RecallRow],
) -> Result<Vec<usize>> {
    use snafu::ResultExt;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let user = render_prompt(new_prompt, candidates);
    let body = serde_json::json!({
        "model": api.model,
        "max_tokens": JUDGE_MAX_TOKENS,
        "system": SYSTEM,
        "messages": [{ "role": "user", "content": user }],
    });

    let resp = api
        .http
        .post(format!("{}/v1/messages", api.api_base))
        .header("x-api-key", api.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .context(JudgeSendSnafu { model: api.model.to_owned() })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return JudgeStatusSnafu { status, body }.fail();
    }

    let parsed: MessagesResponse = resp.json().await.context(JudgeDecodeSnafu)?;
    let text: String = parsed.content.iter().map(|b| b.text.as_str()).collect();
    Ok(parse_indices(&text, candidates.len()))
}

fn render_prompt(new_prompt: &str, candidates: &[RecallRow]) -> String {
    let mut s = String::new();
    s.push_str("NEW question:\n");
    s.push_str(new_prompt);
    s.push_str("\n\nCACHED investigations:\n");
    for (i, c) in candidates.iter().enumerate() {
        let paths: Vec<&str> = c
            .file_deps
            .iter()
            .take(MAX_DEP_PATHS)
            .map(|d| d.path.as_str())
            .collect();
        s.push_str(&(i + 1).to_string());
        s.push_str(". question: ");
        s.push_str(&c.question);
        s.push_str("\n   files: ");
        s.push_str(&paths.join(", "));
        s.push('\n');
    }
    s
}

/// Extract 1-based numbers from the judge reply and map them to in-range 0-based
/// indices, deduped and order-preserving. Fail-closed: junk yields no hits.
fn parse_indices(text: &str, n: usize) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for token in text.split(|c: char| !c.is_ascii_digit()) {
        if token.is_empty() {
            continue;
        }
        if let Ok(num) = token.parse::<usize>()
            && num >= 1
            && num <= n
            && seen.insert(num)
        {
            out.push(num - 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_indices;

    #[test]
    fn parses_comma_list() {
        assert_eq!(parse_indices("1,3", 3), vec![0, 2]);
    }

    #[test]
    fn ignores_out_of_range_and_dupes() {
        assert_eq!(parse_indices("2, 2, 9", 3), vec![1]);
    }

    #[test]
    fn none_yields_empty() {
        assert!(parse_indices("NONE", 3).is_empty());
    }

    #[test]
    fn junk_is_fail_closed() {
        assert!(parse_indices("maybe the first one?", 3).is_empty());
    }
}
