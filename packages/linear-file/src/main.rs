//! File one Linear issue and print its identifier and URL as JSON.
//!
//! The house rules require filing a ticket the moment friction is hit. Before
//! this there was no tool, so each filing was a hand-built GraphQL POST with a
//! heredoc description and a `jq -n` payload: about fifteen lines when correct,
//! and the failure mode of getting it wrong is a corrupted description or a
//! duplicate issue from a blind retry.
//!
//! Everything the shell recipe had to get right is here instead: the key
//! lookup, the raw (not `Bearer`) Authorization header, JSON-safe encoding of a
//! description containing quotes and newlines, resolving label names to ids,
//! and reading the identifier and URL back out of the response.
//!
//! Output is JSON on stdout and nothing else, so a caller can pipe it. Errors
//! go to stderr with a nonzero exit.

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};

/// Team ENG (Engineering). The default because it is where engineering
/// friction goes; `--team` takes any team key or id for the others.
const DEFAULT_TEAM_KEY: &str = "ENG";

const DEFAULT_URL: &str = "https://api.linear.app/graphql";

/// Login Keychain entry, the same one the friction-report hook and ci-triage
/// read, so a machine with either working already works here.
const KEYCHAIN_SERVICE: &str = "pr-watch-linear";

const TIMEOUT: Duration = Duration::from_secs(30);

const NO_KEY: &str = "no Linear API key: pass --api-key, set LINEAR_API_KEY, or add the login \
                      Keychain entry `pr-watch-linear`";

const CREATE: &str = "mutation($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue { id identifier url title }
  }
}";

const TEAM_BY_KEY: &str = "query($key: String!) {
  teams(filter: { key: { eq: $key } }, first: 1) { nodes { id } }
}";

const LABELS_FOR_TEAM: &str = "query($team: String!) {
  team(id: $team) { labels(first: 250) { nodes { id name } } }
}";

const UPDATE: &str = "mutation($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue { id identifier url title }
  }
}";

/// Reads the issue plus the team it belongs to and the labels it already
/// carries. All three are needed for one update: the team to resolve label
/// names, and the current labels to send the union rather than a replacement.
const ISSUE_FOR_UPDATE: &str = "query($id: String!) {
  issue(id: $id) {
    id
    identifier
    team { id }
    labels { nodes { id name } }
  }
}";

#[derive(Parser)]
#[command(
    name = "linear-file",
    about = "File a Linear issue and print its identifier and URL as JSON",
    long_about = "File one Linear issue.\n\n\
                  The description is read from --description, or from \
                  --description-file, or from stdin when neither is given, so a \
                  long body never has to survive shell quoting.\n\n\
                  Labels are given by name and resolved against the team; \
                  `auto-filed` marks a report as first-hand but unreviewed by a \
                  human and belongs on anything an agent files."
)]
struct Args {
    /// Issue title. Required when filing; when amending, given only to change it.
    #[arg(long)]
    title: Option<String>,

    /// Issue description as markdown. Omit to read it from stdin.
    #[arg(long, conflicts_with = "description_file")]
    description: Option<String>,

    /// Read the description from this file. `-` means stdin.
    #[arg(long)]
    description_file: Option<String>,

    /// Amend this issue instead of filing a new one. Takes an identifier
    /// (ENG-1234) or an id.
    #[arg(long, value_name = "ISSUE")]
    update: Option<String>,

    /// Label name, repeatable. Resolved against the team's labels. When
    /// amending, these are ADDED to the labels the issue already has.
    #[arg(long = "label")]
    labels: Vec<String>,

    /// Label name to take off, repeatable. Amending only.
    #[arg(long = "remove-label", requires = "update")]
    remove_labels: Vec<String>,

    /// Make --label the whole label set rather than an addition, dropping any
    /// label not named. Amending only, and needs at least one --label, because
    /// the accident this guards against is wiping the set to nothing.
    #[arg(long, requires = "update", requires = "labels")]
    set_labels: bool,

    /// Team key (such as ENG) or team id.
    #[arg(long, default_value = DEFAULT_TEAM_KEY)]
    team: String,

    /// Project id to file into.
    #[arg(long)]
    project: Option<String>,

    /// Linear API key. Falls back to `LINEAR_API_KEY`, then the login Keychain.
    #[arg(long, env = "LINEAR_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// GraphQL endpoint, for pointing tests at a local stub.
    #[arg(long, env = "LINEAR_API_URL", default_value = DEFAULT_URL)]
    url: String,

    /// Print the request that would be sent and file nothing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Deserialize)]
struct Issue {
    id: String,
    identifier: String,
    url: String,
    title: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("linear-file: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .context("building the HTTP client")?;

    if let Some(issue) = args.update.clone() {
        return amend(&client, &args, &issue);
    }

    let description = description_of(&args)?;
    let mut input = json!({
        "title": args.title.clone().context("--title is required when filing")?,
        "description": description,
    });

    if args.dry_run {
        // Resolving the team and the labels needs the API, and a dry run must
        // not need credentials, so the names are echoed back unresolved.
        let preview = json!({
            "url": args.url,
            "team": args.team,
            "labels": args.labels,
            "project": args.project,
            "input": input,
        });
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }

    let key = api_key(args.api_key.as_deref()).context(NO_KEY)?;

    let team = team_id(&client, &key, &args.url, &args.team)?;
    input["teamId"] = json!(team);
    if let Some(project) = args.project.as_deref() {
        input["projectId"] = json!(project);
    }
    if !args.labels.is_empty() {
        input["labelIds"] = json!(label_ids(&client, &key, &args.url, &team, &args.labels)?);
    }

    let data = post(&client, &key, &args.url, CREATE, &json!({ "input": input }))?;
    let created = data
        .pointer("/issueCreate/issue")
        .cloned()
        .unwrap_or_default();
    let issue: Issue = serde_json::from_value(created)
        .context("issueCreate returned no issue; the input was rejected")?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "id": issue.id,
            "identifier": issue.identifier,
            "url": issue.url,
            "title": issue.title,
        }))?
    );
    Ok(())
}

/// The description, from the flag, a file, or stdin.
///
/// Stdin is the default so a body with quotes, backticks or newlines never has
/// to survive shell quoting, which is what corrupted hand-built payloads.
fn description_of(args: &Args) -> Result<String> {
    // Filing always has a body, so stdin is the default source.
    description_if_given(args)?.map_or_else(read_stdin, Ok)
}

/// The description only where one was actually asked for.
///
/// Amending must not default to stdin: `--update ENG-1 --title x` would then
/// block on a terminal, and under a redirect would overwrite the body with
/// whatever it read. A flag nobody passed has to mean "leave it alone".
fn description_if_given(args: &Args) -> Result<Option<String>> {
    if let Some(text) = args.description.as_deref() {
        return Ok(Some(text.to_owned()));
    }
    match args.description_file.as_deref() {
        None => Ok(None),
        Some("-") => read_stdin().map(Some),
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading {path}"))
            .map(Some),
    }
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("reading the description from stdin")?;
    Ok(buf)
}

/// Amend an existing issue's title, description or labels.
///
/// Labels are read before they are written. `issueUpdate`'s `labelIds`
/// REPLACES the set rather than adding to it, measured on 2026-08-01: setting
/// one label on an issue carrying two dropped the other. So `--label` sends the
/// union with what the issue already has, and taking a label off needs the
/// explicit `--remove-label` or `--set-labels`.
fn amend(client: &reqwest::blocking::Client, args: &Args, issue: &str) -> Result<()> {
    let description = description_if_given(args)?;
    let touches_labels = !args.labels.is_empty() || !args.remove_labels.is_empty();
    if args.title.is_none() && description.is_none() && !touches_labels {
        bail!("nothing to change: give --title, a description, or a label flag");
    }

    if args.dry_run {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "url": args.url,
                "update": issue,
                "title": args.title,
                "description": description,
                "add_labels": args.labels,
                "remove_labels": args.remove_labels,
                "set_labels": args.set_labels,
            }))?
        );
        return Ok(());
    }

    let key = api_key(args.api_key.as_deref()).context(NO_KEY)?;
    let data = post(
        client,
        &key,
        &args.url,
        ISSUE_FOR_UPDATE,
        &json!({ "id": issue }),
    )?;
    let current = data
        .get("issue")
        .filter(|v| !v.is_null())
        .with_context(|| format!("no issue `{issue}`"))?;
    let id = current
        .pointer("/id")
        .and_then(Value::as_str)
        .context("the issue has no id")?;

    let mut input = json!({});
    if let Some(title) = args.title.as_deref() {
        input["title"] = json!(title);
    }
    if let Some(body) = description {
        input["description"] = json!(body);
    }
    if touches_labels {
        let team = current
            .pointer("/team/id")
            .and_then(Value::as_str)
            .context("the issue has no team")?;
        input["labelIds"] = json!(next_labels(client, &key, args, team, current)?);
    }

    let data = post(
        client,
        &key,
        &args.url,
        UPDATE,
        &json!({ "id": id, "input": input }),
    )?;
    let updated: Issue = serde_json::from_value(
        data.pointer("/issueUpdate/issue")
            .cloned()
            .unwrap_or_default(),
    )
    .context("issueUpdate returned no issue; the input was rejected")?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "id": updated.id,
            "identifier": updated.identifier,
            "url": updated.url,
            "title": updated.title,
        }))?
    );
    Ok(())
}

/// The label set to send: the union of what the issue carries with `--label`,
/// less `--remove-label`, or exactly `--label` under `--set-labels`.
fn next_labels(
    client: &reqwest::blocking::Client,
    key: &str,
    args: &Args,
    team: &str,
    current: &Value,
) -> Result<Vec<String>> {
    let added = label_ids(client, key, &args.url, team, &args.labels)?;
    if args.set_labels {
        return Ok(added);
    }
    let removed = label_ids(client, key, &args.url, team, &args.remove_labels)?;
    let existing: Vec<String> = current
        .pointer("/labels/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| n.get("id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(merge_labels(&existing, &added, &removed))
}

/// `existing` plus `added`, less `removed`, order preserved and no duplicates.
///
/// Split out from the network path so the arithmetic behind a destructive
/// write is covered by a test rather than only by a live run.
fn merge_labels(existing: &[String], added: &[String], removed: &[String]) -> Vec<String> {
    let mut next = existing.to_vec();
    for id in added {
        if !next.contains(id) {
            next.push(id.clone());
        }
    }
    next.retain(|id| !removed.contains(id));
    next
}

/// The API key from the flag or environment, else the login Keychain entry the
/// other tools here already use.
fn api_key(flag: Option<&str>) -> Option<String> {
    if let Some(key) = flag.filter(|k| !k.is_empty()) {
        return Some(key.to_owned());
    }
    let out = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let key = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if key.is_empty() { None } else { Some(key) }
}

/// One POST, with the GraphQL `errors` array turned into a real error.
///
/// Linear answers HTTP 200 with an `errors` array for a rejected query, so
/// checking the status alone reports success on a failed filing.
fn post(
    client: &reqwest::blocking::Client,
    key: &str,
    url: &str,
    query: &str,
    variables: &Value,
) -> Result<Value> {
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        // Linear wants the raw key here, NOT `Bearer <key>`.
        .header("Authorization", key)
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .context("POST to the Linear API")?;
    let status = response.status();
    let body: Value = response.json().context("decoding the Linear response")?;
    if let Some(errors) = body.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        let messages: Vec<&str> = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(Value::as_str))
            .collect();
        bail!("Linear rejected the request: {}", messages.join("; "));
    }
    if !status.is_success() {
        bail!("Linear returned HTTP {status}");
    }
    body.get("data")
        .cloned()
        .context("Linear returned no data field")
}

/// The team's id. A value that already looks like an id is passed through, so
/// `--team <uuid>` works without a lookup round-trip.
fn team_id(client: &reqwest::blocking::Client, key: &str, url: &str, team: &str) -> Result<String> {
    if looks_like_an_id(team) {
        return Ok(team.to_owned());
    }
    let data = post(client, key, url, TEAM_BY_KEY, &json!({ "key": team }))?;
    data.pointer("/teams/nodes/0/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("no team with key `{team}`"))
}

/// Label ids for the given names, matched case-insensitively against the
/// team's labels.
///
/// Names rather than ids because a name is what a person knows, and because
/// `issueUpdate`'s labelIds replaces rather than appends, so a wrong id is
/// silently the wrong label rather than an error.
fn label_ids(
    client: &reqwest::blocking::Client,
    key: &str,
    url: &str,
    team: &str,
    names: &[String],
) -> Result<Vec<String>> {
    let data = post(client, key, url, LABELS_FOR_TEAM, &json!({ "team": team }))?;
    let nodes = data
        .pointer("/team/labels/nodes")
        .and_then(Value::as_array)
        .context("the team returned no labels")?;
    names
        .iter()
        .map(|wanted| {
            if looks_like_an_id(wanted) {
                return Ok(wanted.clone());
            }
            nodes
                .iter()
                .find(|node| {
                    node.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
                })
                .and_then(|node| node.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .with_context(|| format!("no label named `{wanted}` on this team"))
        })
        .collect()
}

/// Whether a value is already a Linear id (a UUID) rather than a key or name.
fn looks_like_an_id(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::{looks_like_an_id, merge_labels};

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// `issueUpdate`'s `labelIds` replaces the set rather than adding to it,
    /// measured against the API on 2026-08-01: setting one label on an issue
    /// carrying two dropped the other. So an add has to send the union, or
    /// labelling an issue silently strips every label it already had.
    #[test]
    fn adding_a_label_keeps_the_ones_already_there() {
        assert_eq!(
            merge_labels(&ids(&["a"]), &ids(&["b"]), &[]),
            ids(&["a", "b"])
        );
        // Adding one it already carries is not a duplicate.
        assert_eq!(merge_labels(&ids(&["a"]), &ids(&["a"]), &[]), ids(&["a"]));
        // Removal takes precedence, including of something added in the same call.
        assert_eq!(
            merge_labels(&ids(&["a", "b"]), &[], &ids(&["a"])),
            ids(&["b"])
        );
        assert_eq!(
            merge_labels(&ids(&["a"]), &ids(&["b"]), &ids(&["b"])),
            ids(&["a"])
        );
        // Removing something absent is not an error and changes nothing.
        assert_eq!(merge_labels(&ids(&["a"]), &[], &ids(&["z"])), ids(&["a"]));
        // No flags, no change.
        assert_eq!(merge_labels(&ids(&["a", "b"]), &[], &[]), ids(&["a", "b"]));
    }

    #[test]
    fn an_id_is_told_apart_from_a_key_or_a_name() {
        assert!(looks_like_an_id("a8845362-21c7-4283-ba80-cea987a3ee74"));
        assert!(looks_like_an_id("e34246ae-7b01-46a4-b855-747d7222c34d"));
        // Team keys and label names must not be mistaken for ids, or the
        // lookup is skipped and the API rejects the filing.
        assert!(!looks_like_an_id("ENG"));
        assert!(!looks_like_an_id("auto-filed"));
        assert!(!looks_like_an_id(""));
        // Right length, wrong shape.
        assert!(!looks_like_an_id("a8845362-21c7-4283-ba80-cea987a3eeZZ"));
        assert!(!looks_like_an_id("a884536221c74283ba80cea987a3ee7412345"));
    }
}
