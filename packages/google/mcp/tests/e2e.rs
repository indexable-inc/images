//! End-to-end: drive the real binary over stdio and read what a host reads.
//!
//! These assert the behaviour the unit tests cannot. The claim being
//! defended is "an unconfigured host still gets a usable server that
//! explains itself", and the only honest way to check it is to run the
//! process with nothing set up and speak MCP to it. A type-level test would
//! have passed just as happily against the old code that exited during
//! startup.
//!
//! Every case runs against a scratch `HOME`, so a developer's real
//! `token.json` can neither make a test pass nor leak into one.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// Where `dirs::config_dir` will look, given a `HOME`.
///
/// macOS ignores `XDG_CONFIG_HOME`, so the two platforms are computed
/// separately rather than assuming the Linux layout.
fn config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".config")
    }
}

/// One MCP session: send `requests`, return the decoded responses.
///
/// The child inherits nothing (`env_clear`), so the ambient environment --
/// including a real `GOOGLE_OAUTH_CLIENT_ID` on a developer's machine --
/// cannot change the outcome.
/// One server run as a host sees it: the JSON-RPC messages read off stdout,
/// and the whole of stderr. Named rather than returned as a pair so a caller
/// cannot bind the two the wrong way round.
struct Session {
    responses: Vec<Value>,
    stderr: String,
}

fn session(home: &Path, env: &[(&str, &str)], requests: &str) -> Session {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ix-google-mcp"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the server binary starts");

    // JSON-RPC over stdio is line-framed: one message per line. A raw string
    // wrapped across source lines silently becomes two malformed messages and
    // shows up much later as a missing response, so check it at the source.
    for line in requests.lines().filter(|line| !line.trim().is_empty()) {
        assert!(
            serde_json::from_str::<Value>(line).is_ok(),
            "each request must be exactly one line of JSON: {line}"
        );
    }

    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(requests.as_bytes())
        .expect("writes the requests");

    let output = child.wait_with_output().expect("the server exits cleanly");
    assert!(
        output.status.success(),
        "the server must exit 0 even when nothing is configured; got {:?}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let responses = String::from_utf8(output.stdout)
        .expect("stdout is UTF-8")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each line is one JSON-RPC message"))
        .collect();
    Session {
        responses,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

const HANDSHAKE: &str = concat!(
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","#,
    r#""capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#,
    "\n",
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    "\n",
);

fn find(responses: &[Value], id: i64) -> &Value {
    responses
        .iter()
        .find(|message| message.get("id").and_then(Value::as_i64) == Some(id))
        .unwrap_or_else(|| panic!("no response with id {id}: {responses:#?}"))
}

fn instructions(responses: &[Value]) -> String {
    find(responses, 1)["result"]["instructions"]
        .as_str()
        .expect("initialize carries instructions")
        .to_owned()
}

fn write_client_secret(home: &Path) {
    let dir = config_dir(home).join("google");
    std::fs::create_dir_all(&dir).expect("creates the config dir");
    // Exactly the shape the Cloud Console hands an outside user.
    std::fs::write(
        dir.join("client_secret.json"),
        r#"{"installed":{"client_id":"outsider.apps.googleusercontent.com",
            "client_secret":"OUTSIDER-SECRET","redirect_uris":["http://localhost"]}}"#,
    )
    .expect("writes the client secret");
}

#[test]
fn an_unconfigured_server_completes_the_handshake_and_says_what_is_missing() {
    let home = TempDir::new().expect("temp home");

    let Session { responses, stderr } = session(home.path(), &[], HANDSHAKE);

    // The regression this exists for: the old build exited during startup,
    // so there was no response at all.
    let instructions = instructions(&responses);
    for capability in ["mail.read", "mail.send", "calendar"] {
        assert!(
            instructions.contains(capability),
            "every capability must be accounted for; missing {capability} in:\n{instructions}"
        );
    }
    assert!(
        instructions.contains("no OAuth client"),
        "the agent must learn the actual cause:\n{instructions}"
    );
    assert!(
        instructions.contains("they cannot see this"),
        "the agent must be told to relay it, since the user never sees instructions"
    );
    // Second channel: the human reads stderr, not the model's context.
    assert!(
        stderr.contains("Google integration is not working"),
        "stderr must carry the report too:\n{stderr}"
    );
}

#[test]
fn every_tool_including_google_status_is_listed_while_unconfigured() {
    let home = TempDir::new().expect("temp home");
    let requests = format!(
        "{HANDSHAKE}{}\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#
    );

    let Session { responses, .. } = session(home.path(), &[], &requests);

    let tools = find(&responses, 2)["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        names.contains(&"google_status"),
        "the tool that explains the setup must exist while unconfigured: {names:?}"
    );
    assert!(
        names.contains(&"mail_send_message"),
        "an unconfigured server must still advertise its surface, or the agent \
         cannot tell a setup problem from a missing feature: {names:?}"
    );
}

#[test]
fn a_tool_call_while_unconfigured_names_the_cause_and_the_next_step() {
    let home = TempDir::new().expect("temp home");
    let requests = format!(
        "{HANDSHAKE}{}\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mail_search","arguments":{"query":"is:unread"}}}"#
    );

    let Session { responses, .. } = session(home.path(), &[], &requests);

    let message = serde_json::to_string(find(&responses, 2)).expect("serializes");
    assert!(
        message.contains("google_status") || message.contains("gmail auth"),
        "a failure must point at the fix, not just report a failure: {message}"
    );
}

#[test]
fn a_malformed_call_reports_the_schema_error_not_the_setup_error() {
    let home = TempDir::new().expect("temp home");
    let requests = format!(
        "{HANDSHAKE}{}\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mail_search","arguments":{"q":"is:unread"}}}"#
    );

    let Session { responses, .. } = session(home.path(), &[], &requests);

    // `q` is not the field name; the caller needs to hear that, not "the
    // server is not set up". An unconfigured server that blamed its own
    // configuration for every failure would send agents fixing the wrong thing.
    let message = serde_json::to_string(find(&responses, 2)).expect("serializes");
    assert!(
        message.contains("query"),
        "a schema error must name the field, even while unconfigured: {message}"
    );
}

#[test]
fn a_downloaded_client_secret_is_picked_up_without_any_environment() {
    let home = TempDir::new().expect("temp home");
    write_client_secret(home.path());

    let Session { responses, .. } = session(home.path(), &[], HANDSHAKE);

    // The whole bring-your-own-client claim: no env vars anywhere, and the
    // server has moved past "no OAuth client" to "nobody has consented".
    let instructions = instructions(&responses);
    assert!(
        !instructions.contains("no OAuth client"),
        "a dropped-in client_secret.json must count as configured:\n{instructions}"
    );
    assert!(
        instructions.contains("gmail auth"),
        "the remaining step is consent, and it must be named:\n{instructions}"
    );
}

#[test]
fn smtp_alone_makes_sending_work_with_no_google_configuration() {
    let home = TempDir::new().expect("temp home");

    let Session { responses, .. } = session(
        home.path(),
        &[
            ("IX_SMTP_HOST", "smtp.fastmail.com"),
            ("IX_SMTP_USER", "someone@fastmail.com"),
            ("IX_SMTP_PASSWORD", "hunter2-app-password"),
        ],
        HANDSHAKE,
    );

    let instructions = instructions(&responses);
    assert!(
        instructions.contains("ok   mail.send"),
        "SMTP satisfies sending on its own:\n{instructions}"
    );
    assert!(
        instructions.contains("smtp.fastmail.com"),
        "the report should name the endpoint so the user can check it:\n{instructions}"
    );
    // Reading still needs Google; conflating the two is the failure mode
    // this whole health split exists to prevent.
    assert!(
        instructions.contains("todo mail.read"),
        "a working sender must not imply a readable mailbox:\n{instructions}"
    );
}

#[test]
fn the_smtp_password_never_appears_in_anything_the_server_emits() {
    let home = TempDir::new().expect("temp home");
    let password = "super-secret-app-password";

    let Session { responses, stderr } = session(
        home.path(),
        &[
            ("IX_SMTP_HOST", "smtp.fastmail.com"),
            ("IX_SMTP_USER", "someone@fastmail.com"),
            ("IX_SMTP_PASSWORD", password),
        ],
        HANDSHAKE,
    );

    let stdout = serde_json::to_string(&responses).expect("serializes");
    assert!(
        !stdout.contains(password),
        "the credential reached the wire"
    );
    assert!(
        !stderr.contains(password),
        "the credential reached the logs"
    );
}
