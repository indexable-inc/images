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

use crate::common::TestEnvironment;

/// Template that renders only the head operation's attributes.
const ATTRIBUTES_TEMPLATE: &str = r#"attributes ++ "\n""#;

#[test]
fn test_agent_attributes_from_env() {
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("JJ_AGENT_KIND", "claude-code");
    test_env.add_env_var("JJ_AGENT_SESSION", "sess-0123");
    // Absolute on the way in: the attribute must come out as a file name, and
    // that redaction is also what makes this snapshot machine-independent.
    let transcript = test_env.env_root().join("repo").join("transcript.jsonl");
    test_env.add_env_var("JJ_AGENT_TRANSCRIPT", transcript.to_str().unwrap().to_owned());
    test_env.add_env_var("JJ_AGENT_MODEL", "fable-5");
    test_env.add_env_var("JJ_AGENT_EFFORT", "high");
    test_env.add_env_var("JJ_AGENT_VERSION", "2.1.0");
    test_env.add_env_var("JJ_AGENT_PARENT", "sess-parent");
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");
    // 12 bytes; agent.turn_offset must be the transcript's byte length, taken
    // from the unredacted path even though the attribute is redacted.
    work_dir.write_file("transcript.jsonl", "twelve bytes");

    work_dir.run_jj(["describe", "-m", "stamped"]).success();

    let output = work_dir.run_jj(["op", "log", "-n1", "--no-graph", "-T", ATTRIBUTES_TEMPLATE]);
    insta::assert_snapshot!(output, @r"
    agent.effort: high
    agent.kind: claude-code
    agent.model: fable-5
    agent.parent: sess-parent
    agent.session: sess-0123
    agent.transcript: transcript.jsonl
    agent.turn_offset: 12
    agent.version: 2.1.0
    args: jj describe -m stamped
    [EOF]
    ");
}

#[test]
fn test_agent_attributes_env_defaults() {
    // JJ_AGENT_SESSION alone is what makes the context present; kind
    // defaults to "unknown" and absent fields are omitted (including
    // agent.turn_offset when the transcript cannot be stat'ed).
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("JJ_AGENT_SESSION", "sess-solo");
    test_env.add_env_var("JJ_AGENT_TRANSCRIPT", "/nonexistent/transcript.jsonl");
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    work_dir.run_jj(["describe", "-m", "stamped"]).success();

    let output = work_dir.run_jj(["op", "log", "-n1", "--no-graph", "-T", ATTRIBUTES_TEMPLATE]);
    insta::assert_snapshot!(output, @r"
    agent.kind: unknown
    agent.session: sess-solo
    agent.transcript: transcript.jsonl
    args: jj describe -m stamped
    [EOF]
    ");
}

#[test]
fn test_no_agent_attributes_without_env() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    work_dir.run_jj(["describe", "-m", "stamped"]).success();

    let output = work_dir.run_jj(["op", "log", "-n1", "--no-graph", "-T", ATTRIBUTES_TEMPLATE]);
    // Positive control: the args attribute proves the template renders
    // attributes at all; and no agent.* keys appear.
    insta::assert_snapshot!(output, @r"
    args: jj describe -m stamped
    [EOF]
    ");
    assert!(!output.stdout.raw().contains("agent."));
}

#[test]
fn test_agent_attributes_from_claude_code_fallback() {
    let mut test_env = TestEnvironment::default();
    let map_dir = test_env.env_root().join("session-by-pid");
    std::fs::create_dir_all(&map_dir).unwrap();
    std::fs::write(
        map_dir.join("12345.json"),
        r#"{"session_id": "sess-fallback", "model": "fable-5", "transcript": "/nonexistent/transcript.jsonl"}"#,
    )
    .unwrap();
    test_env.add_env_var("CLAUDE_CODE_MESSAGING_SOCKET", "/tmp/cc-socks/12345.sock");
    test_env.add_env_var("JJ_AGENT_MAP_DIR", map_dir.to_str().unwrap().to_owned());
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    work_dir.run_jj(["describe", "-m", "stamped"]).success();

    let output = work_dir.run_jj(["op", "log", "-n1", "--no-graph", "-T", ATTRIBUTES_TEMPLATE]);
    insta::assert_snapshot!(output, @r"
    agent.kind: claude-code
    agent.model: fable-5
    agent.session: sess-fallback
    agent.transcript: transcript.jsonl
    args: jj describe -m stamped
    [EOF]
    ");
}

#[test]
fn test_agent_attributes_fallback_malformed_is_silent() {
    // A missing or malformed pid-map file must mean "no context", never an
    // error: jj must keep working for everyone.
    let mut test_env = TestEnvironment::default();
    let map_dir = test_env.env_root().join("session-by-pid");
    std::fs::create_dir_all(&map_dir).unwrap();
    std::fs::write(map_dir.join("12345.json"), "not json at all {").unwrap();
    test_env.add_env_var("CLAUDE_CODE_MESSAGING_SOCKET", "/tmp/cc-socks/12345.sock");
    test_env.add_env_var("JJ_AGENT_MAP_DIR", map_dir.to_str().unwrap().to_owned());
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    work_dir.run_jj(["describe", "-m", "stamped"]).success();

    let output = work_dir.run_jj(["op", "log", "-n1", "--no-graph", "-T", ATTRIBUTES_TEMPLATE]);
    insta::assert_snapshot!(output, @r"
    args: jj describe -m stamped
    [EOF]
    ");
    assert!(!output.stdout.raw().contains("agent."));
}

#[test]
fn test_agent_transcript_without_a_directory_is_unchanged() {
    // Negative arm at the wire-facing site: a value with no directory in it
    // must reach the operation attribute byte-for-byte. Without this the
    // redaction tests would still pass if the attribute were replaced by a
    // constant, and a producer that already redacts would silently lose the
    // only identifier the attribute carries.
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("JJ_AGENT_SESSION", "sess-bare");
    test_env.add_env_var("JJ_AGENT_TRANSCRIPT", "7259a481-uuid.jsonl");
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    work_dir.run_jj(["describe", "-m", "stamped"]).success();

    let output = work_dir.run_jj(["op", "log", "-n1", "--no-graph", "-T", ATTRIBUTES_TEMPLATE]);
    insta::assert_snapshot!(output, @r"
    agent.kind: unknown
    agent.session: sess-bare
    agent.transcript: 7259a481-uuid.jsonl
    args: jj describe -m stamped
    [EOF]
    ");
}

#[test]
fn test_agent_transcript_attribute_never_carries_a_directory() {
    // The invariant the leak is about, asserted on rendered output rather
    // than on a return value: whatever a producer supplies, no operation
    // attribute may name a directory. Deep path in, file name out.
    let mut test_env = TestEnvironment::default();
    test_env.add_env_var("JJ_AGENT_SESSION", "sess-deep");
    test_env.add_env_var(
        "JJ_AGENT_TRANSCRIPT",
        "/one/two/three/four/five/deep-uuid.jsonl",
    );
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    work_dir.run_jj(["describe", "-m", "stamped"]).success();

    let output = work_dir.run_jj(["op", "log", "-n1", "--no-graph", "-T", ATTRIBUTES_TEMPLATE]);
    let rendered = output.stdout.raw();
    assert!(
        rendered.contains("agent.transcript: deep-uuid.jsonl"),
        "expected the file name only, got: {rendered}"
    );
    for segment in ["/one", "two/", "three", "four", "five"] {
        assert!(
            !rendered.contains(segment),
            "operation attributes leaked path segment {segment:?}: {rendered}"
        );
    }
}
