//! Behavioural tests for the journald adapter: per-(unit, day) grouping, the
//! priority filter, unit attribution fallbacks, document caps, and sanitation.

use source_journald::{JournaldLog, MAX_BODY_BYTES, MAX_MESSAGES, SOURCE_TAG};
use source_meta::SourceAdapter as _;

/// One `journalctl -o json` line, in the captured field shape (every value is a
/// string except the byte-array `MESSAGE` form, tested separately).
fn line(unit_field: &str, unit: &str, priority: u8, micros: i64, message: &str) -> String {
    format!(
        r#"{{"PRIORITY":"{priority}","MESSAGE":{message},"{unit_field}":"{unit}","__REALTIME_TIMESTAMP":"{micros}","_TRANSPORT":"journal"}}"#,
        message = serde_json::json!(message),
    )
}

/// Epoch microseconds for a second offset into 2026-06-10 UTC.
const DAY_A: i64 = 1_781_049_600_000_000; // 2026-06-10T00:00:00Z
/// Epoch microseconds for 2026-06-11 UTC.
const DAY_B: i64 = 1_781_136_000_000_000; // 2026-06-11T00:00:00Z

#[test]
fn groups_per_unit_and_day_with_filter_tags() {
    let input = [
        // nginx: two messages on day A, one on day B -> two documents.
        line("_SYSTEMD_UNIT", "nginx.service", 3, DAY_A + 60_000_000, "worker died"),
        line("_SYSTEMD_UNIT", "nginx.service", 4, DAY_A + 5_000_000, "slow upstream"),
        line("_SYSTEMD_UNIT", "nginx.service", 3, DAY_B + 1_000_000, "worker died again"),
        // Another unit, same day A -> its own document.
        line("_SYSTEMD_UNIT", "ix-indexer.service", 2, DAY_A + 90_000_000, "sync crashed"),
        // priority 6 (info) must be dropped even if present in a capture.
        line("_SYSTEMD_UNIT", "nginx.service", 6, DAY_A + 70_000_000, "started ok"),
    ]
    .join("\n");

    let log = JournaldLog::parse(input.as_bytes(), "hil-compute-1").expect("parse");
    let docs: Vec<_> = log.documents().map(|d| d.expect("doc")).collect();
    assert_eq!(docs.len(), 3, "two nginx days plus one indexer day");

    let nginx_a = docs
        .iter()
        .find(|d| d.external_id == "journald:hil-compute-1:nginx.service:2026-06-10")
        .expect("nginx day A doc");
    assert_eq!(nginx_a.meta_json["source"], SOURCE_TAG);
    assert_eq!(nginx_a.meta_json["host"], "hil-compute-1");
    assert_eq!(nginx_a.meta_json["unit"], "nginx.service");
    assert_eq!(
        nginx_a.meta_json["title"],
        "nginx.service warnings/errors 2026-06-10"
    );
    // Recency axis: the day's LAST message (00:01:00, not the 00:00:05 one).
    assert_eq!(nginx_a.meta_json["timestamp"], (DAY_A + 60_000_000) / 1_000_000);

    let body = String::from_utf8(nginx_a.body.clone()).expect("utf8");
    // Messages are time-sorted even though the input was not, with level labels.
    assert_eq!(
        body,
        "00:00:05 [warning] slow upstream\n00:01:00 [err] worker died\n"
    );
    assert!(
        !body.contains("started ok"),
        "info-level lines must not be embedded"
    );

    assert!(
        docs.iter()
            .any(|d| d.external_id == "journald:hil-compute-1:nginx.service:2026-06-11"),
        "day B is its own document"
    );
    assert!(
        docs.iter()
            .any(|d| d.external_id == "journald:hil-compute-1:ix-indexer.service:2026-06-10"),
        "the second unit is its own document"
    );
}

#[test]
fn unit_attribution_falls_back_through_the_field_chain() {
    let input = [
        // pid1 talks ABOUT a unit via UNIT (no _SYSTEMD_UNIT on the entry).
        line("UNIT", "minio.service", 3, DAY_A + 1_000_000, "Failed to start"),
        // kernel messages carry only a syslog identifier.
        line("SYSLOG_IDENTIFIER", "kernel", 4, DAY_A + 2_000_000, "thermal event"),
        // No attribution at all -> "unknown".
        format!(
            r#"{{"PRIORITY":"3","MESSAGE":"orphan","__REALTIME_TIMESTAMP":"{}"}}"#,
            DAY_A + 3_000_000
        ),
    ]
    .join("\n");

    let log = JournaldLog::parse(input.as_bytes(), "h").expect("parse");
    let mut ids: Vec<_> = log
        .documents()
        .map(|d| d.expect("doc").external_id)
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "journald:h:kernel:2026-06-10",
            "journald:h:minio.service:2026-06-10",
            "journald:h:unknown:2026-06-10",
        ]
    );
}

#[test]
fn byte_array_message_is_recovered_lossily() {
    // journald stores non-UTF-8 payloads as a JSON byte array.
    let input = format!(
        r#"{{"PRIORITY":"3","MESSAGE":[104,105,32,255],"_SYSTEMD_UNIT":"a.service","__REALTIME_TIMESTAMP":"{}"}}"#,
        DAY_A + 1_000_000
    );
    let log = JournaldLog::parse(input.as_bytes(), "h").expect("parse");
    let docs: Vec<_> = log.documents().map(|d| d.expect("doc")).collect();
    let body = String::from_utf8(docs[0].body.clone()).expect("utf8");
    assert!(body.contains("hi \u{fffd}"), "lossy recovery: {body}");
}

#[test]
fn malformed_json_line_is_an_error_not_a_silent_drop() {
    assert!(JournaldLog::parse(b"{not json}\n", "h").is_err());
}

#[test]
fn lines_missing_priority_or_timestamp_are_skipped() {
    let input = [
        r#"{"MESSAGE":"no priority","_SYSTEMD_UNIT":"a.service","__REALTIME_TIMESTAMP":"1"}"#.to_owned(),
        r#"{"PRIORITY":"3","MESSAGE":"no timestamp","_SYSTEMD_UNIT":"a.service"}"#.to_owned(),
    ]
    .join("\n");
    let log = JournaldLog::parse(input.as_bytes(), "h").expect("parse");
    assert!(log.is_empty(), "journal oddities are skipped by design");
}

#[test]
fn message_cap_truncates_with_a_counting_trailer() {
    let total = i64::try_from(MAX_MESSAGES + 50).expect("small");
    let lines: Vec<String> = (0..total)
        .map(|i| {
            line(
                "_SYSTEMD_UNIT",
                "loop.service",
                3,
                DAY_A + i * 1_000_000,
                &format!("crash {i}"),
            )
        })
        .collect();
    let log = JournaldLog::parse(lines.join("\n").as_bytes(), "h").expect("parse");
    let docs: Vec<_> = log.documents().map(|d| d.expect("doc")).collect();
    assert_eq!(docs.len(), 1);

    let body = String::from_utf8(docs[0].body.clone()).expect("utf8");
    assert_eq!(
        body.lines().count(),
        MAX_MESSAGES + 1,
        "200 messages plus the trailer"
    );
    assert!(body.contains("crash 0\n"), "head kept");
    assert!(body.ends_with("[truncated: 50 more messages]\n"), "{body}");
    assert!(!body.contains("crash 230"), "over-cap repeats dropped");
}

#[test]
fn byte_cap_bounds_a_unit_with_few_huge_messages() {
    // 30 messages of ~1.2 KB of prose-shaped words (whitespace keeps the
    // sanitizer's blob collapse away): under the message cap, over the byte cap.
    let big = "lorem ipsum dolor sit amet ".repeat(45);
    let lines: Vec<String> = (0..30i64)
        .map(|i| {
            line(
                "_SYSTEMD_UNIT",
                "big.service",
                3,
                DAY_A + i * 1_000_000,
                &format!("payload {i} {big}"),
            )
        })
        .collect();
    let log = JournaldLog::parse(lines.join("\n").as_bytes(), "h").expect("parse");
    let docs: Vec<_> = log.documents().map(|d| d.expect("doc")).collect();
    let body = String::from_utf8(docs[0].body.clone()).expect("utf8");
    assert!(
        body.len() <= MAX_BODY_BYTES + 64,
        "body stays near the byte cap, got {}",
        body.len()
    );
    assert!(body.contains("[truncated:"), "{body}");
}

#[test]
fn message_text_is_sanitized_before_hashing() {
    // Constructed at test time; never a real credential.
    let fake_token = format!("ghp_{}", "Ab1".repeat(12));
    let input = line(
        "_SYSTEMD_UNIT",
        "leaky.service",
        3,
        DAY_A + 1_000_000,
        &format!("\u{1b}[31mauth failed\u{1b}[0m token={fake_token}"),
    );
    let log = JournaldLog::parse(input.as_bytes(), "h").expect("parse");
    let docs: Vec<_> = log.documents().map(|d| d.expect("doc")).collect();
    let body = String::from_utf8(docs[0].body.clone()).expect("utf8");
    assert!(!body.contains(&fake_token), "raw token must not embed: {body}");
    assert!(body.contains("[redacted:github_token]"), "{body}");
    assert!(!body.contains('\u{1b}'), "ANSI stripped: {body}");
    assert_eq!(
        docs[0].content_hash,
        source_meta::hash_body(&docs[0].body),
        "content_hash covers the sanitized bytes"
    );
}
