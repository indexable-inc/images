//! Report math and rendering: interval union under overlap, and the Markdown
//! surface the sticky comment mirrors.

use net_trace::proxy::{Connection, Scheme};
use net_trace::report::{Phase, markdown, summarize};

fn connection(start_ms: u64, dur_ms: u64, host: &str) -> Connection {
    Connection {
        start_ms,
        dur_ms,
        host: host.to_owned(),
        port: 443,
        scheme: Scheme::Connect,
        bytes_up: 100,
        bytes_down: 1000,
        failed: false,
        finished: true,
    }
}

fn phase(connections: Vec<Connection>) -> Phase {
    Phase {
        label: "required-check".to_owned(),
        cmd: vec!["nix".to_owned()],
        started_at_ms: 0,
        wall_ms: 60_000,
        exit_code: Some(0),
        connections,
    }
}

#[test]
fn overlapping_connections_count_once_in_network_wall() {
    // [0,1000) and [500,1500) overlap: union is 1500, sum would be 2000.
    // [3000,3100) is disjoint.
    let summary = summarize(&[phase(vec![
        connection(0, 1000, "github.com"),
        connection(500, 1000, "github.com"),
        connection(3000, 100, "cache.ix.dev"),
    ])]);
    assert_eq!(summary.phases[0].network_wall_ms, 1600);
    assert_eq!(summary.phases[0].connections, 3);
    assert_eq!(summary.phases[0].bytes_down, 3000);
}

#[test]
fn markdown_carries_marker_hosts_and_caveat() {
    let text = markdown(&summarize(&[phase(vec![connection(
        0,
        1000,
        "github.com",
    )])]));
    assert!(text.starts_with("<!-- net-trace -->"));
    assert!(text.contains("github.com:443"));
    assert!(text.contains("required-check"));
    assert!(text.contains("Daemon-side"));
}

#[test]
fn empty_phase_renders_clean_table() {
    let text = markdown(&summarize(&[phase(vec![])]));
    assert!(text.contains("| required-check |"));
    assert!(text.contains("| 0 |"));
}
