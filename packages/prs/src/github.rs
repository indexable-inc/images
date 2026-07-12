//! Batched GitHub GraphQL lookup of PR state, CI rollup, review decision, and
//! unresolved review threads.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::time::Duration;

use color_eyre::eyre::{Result, WrapErr, eyre};
use serde_json::{Value, json};

use crate::model::{CiState, PrRef, PrState, PrStatus, ReviewState};

/// PRs per GraphQL request; aliases keep it to one round trip per chunk.
const CHUNK: usize = 40;

/// Review threads fetched per PR; a fuller page marks the unresolved count as
/// a lower bound rather than paginating.
const THREAD_PAGE: usize = 100;

/// Find a token: `GH_TOKEN` / `GITHUB_TOKEN`, else `gh auth token`. `None`
/// means the caller degrades to showing patches without live status.
// clone:ignore -- the idiomatic env-then-`gh auth token` bootstrap; resembles
// git-log-pretty's avatar fetcher, which has no shareable library home.
pub fn token() -> Option<String> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(var) {
            let value = value.trim().to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn pr_field(pr: &PrRef, alias: &str) -> String {
    let PrRef {
        owner,
        repo,
        number,
        ..
    } = pr;
    format!(
        r#"{alias}: repository(owner: "{owner}", name: "{repo}") {{
          pullRequest(number: {number}) {{
            state
            isDraft
            reviewDecision
            commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }}
            reviewThreads(first: {THREAD_PAGE}) {{
              pageInfo {{ hasNextPage }}
              nodes {{ isResolved }}
            }}
          }}
        }}"#
    )
}

fn parse_state(pr: &Value) -> Option<PrState> {
    match pr["state"].as_str()? {
        "MERGED" => Some(PrState::Merged),
        "CLOSED" => Some(PrState::Closed),
        "OPEN" if pr["isDraft"].as_bool() == Some(true) => Some(PrState::Draft),
        "OPEN" => Some(PrState::Open),
        _ => None,
    }
}

fn parse_ci(pr: &Value) -> Option<CiState> {
    let rollup = &pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"];
    match rollup["state"].as_str()? {
        "SUCCESS" => Some(CiState::Passing),
        "FAILURE" | "ERROR" => Some(CiState::Failing),
        "PENDING" | "EXPECTED" => Some(CiState::Pending),
        _ => None,
    }
}

fn parse_review(pr: &Value) -> Option<ReviewState> {
    match pr["reviewDecision"].as_str()? {
        "APPROVED" => Some(ReviewState::Approved),
        "CHANGES_REQUESTED" => Some(ReviewState::ChangesRequested),
        "REVIEW_REQUIRED" => Some(ReviewState::ReviewRequired),
        _ => None,
    }
}

fn parse_status(pr: &Value) -> Option<PrStatus> {
    let threads = &pr["reviewThreads"];
    let unresolved = threads["nodes"].as_array().map_or(0, |nodes| {
        nodes
            .iter()
            .filter(|node| node["isResolved"].as_bool() == Some(false))
            .count()
    });
    Some(PrStatus {
        state: parse_state(pr)?,
        ci: parse_ci(pr),
        review: parse_review(pr),
        unresolved,
        unresolved_truncated: threads["pageInfo"]["hasNextPage"].as_bool() == Some(true),
    })
}

/// Fetch status for every distinct PR, keyed by URL. A PR the API cannot
/// return (deleted repo, insufficient scope) is simply absent from the map.
pub fn fetch(prs: &[PrRef], token: &str) -> Result<BTreeMap<String, PrStatus>> {
    let mut seen = BTreeSet::new();
    let distinct: Vec<&PrRef> = prs
        .iter()
        .filter(|pr| seen.insert(pr.url.clone()))
        .collect();
    let client = reqwest::blocking::Client::builder()
        .user_agent("index-prs")
        .timeout(Duration::from_secs(30))
        .build()
        .wrap_err("building HTTP client")?;
    let mut statuses = BTreeMap::new();
    for chunk in distinct.chunks(CHUNK) {
        let fields: Vec<String> = chunk
            .iter()
            .enumerate()
            .map(|(index, pr)| pr_field(pr, &format!("pr{index}")))
            .collect();
        let query = format!("query {{\n{}\n}}", fields.join("\n"));
        let response = client
            .post("https://api.github.com/graphql")
            .bearer_auth(token)
            .json(&json!({ "query": query }))
            .send()
            .wrap_err("querying the GitHub GraphQL API")?;
        let http_status = response.status();
        if !http_status.is_success() {
            return Err(eyre!("GitHub GraphQL API returned {http_status}"));
        }
        let body: Value = response.json().wrap_err("decoding GraphQL response")?;
        for (index, pr) in chunk.iter().enumerate() {
            let node = &body["data"][format!("pr{index}")]["pullRequest"];
            if let Some(status) = parse_status(node) {
                statuses.insert(pr.url.clone(), status);
            }
        }
    }
    Ok(statuses)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_status;
    use crate::model::{CiState, PrState, ReviewState};

    #[test]
    fn parses_full_status() {
        let node = json!({
            "state": "OPEN",
            "isDraft": true,
            "reviewDecision": "CHANGES_REQUESTED",
            "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "state": "FAILURE" } } } ] },
            "reviewThreads": {
                "pageInfo": { "hasNextPage": false },
                "nodes": [ { "isResolved": false }, { "isResolved": true }, { "isResolved": false } ]
            }
        });
        let status = parse_status(&node).expect("status");
        assert_eq!(status.state, PrState::Draft);
        assert_eq!(status.ci, Some(CiState::Failing));
        assert_eq!(status.review, Some(ReviewState::ChangesRequested));
        assert_eq!(status.unresolved, 2);
        assert!(!status.unresolved_truncated);
    }

    #[test]
    fn merged_pr_without_rollup_or_reviews() {
        let node = json!({
            "state": "MERGED",
            "isDraft": false,
            "reviewDecision": null,
            "commits": { "nodes": [] },
            "reviewThreads": { "pageInfo": { "hasNextPage": true }, "nodes": [] }
        });
        let status = parse_status(&node).expect("status");
        assert_eq!(status.state, PrState::Merged);
        assert_eq!(status.ci, None);
        assert_eq!(status.review, None);
        assert_eq!(status.unresolved, 0);
        assert!(status.unresolved_truncated);
    }

    #[test]
    fn missing_pr_yields_none() {
        assert!(parse_status(&serde_json::Value::Null).is_none());
    }
}
