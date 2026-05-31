//! End-to-end producer wire test: a real PTY's rendered screen reaches a reader
//! (the aggregator's role) over the unix socket as a `ProducerSnapshot` of panes.
//!
//! Compiled only with the `dashboard` + `publish` features, the same pair the
//! aggregator builds with. Without them the file is empty.
//!
//! The manager's blocking spawn/write run outside any runtime; the producer and
//! the reader run inside an explicit runtime. `TuiManager` owns its own runtime,
//! so a `#[tokio::test]` would panic when its blocking calls `block_on` from
//! within the test's runtime.
#![cfg(all(feature = "dashboard", feature = "publish"))]

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::UnixStream;
use tui::{ProducerSnapshot, SpawnConfig, TerminalView, TuiManager, View, publish};

/// The first terminal pane in a snapshot, as `(id, view)`. The producer wraps
/// every terminal as a `Pane` whose body is a `View::Terminal`.
fn first_terminal(snapshot: &ProducerSnapshot) -> Option<(String, TerminalView)> {
    snapshot.panes.first().and_then(|pane| match &pane.view {
        View::Terminal(view) => Some((pane.id.clone(), view.clone())),
        _ => None,
    })
}

#[test]
fn producer_streams_live_terminal_over_socket() {
    let manager = Arc::new(TuiManager::new());
    let term = manager
        .spawn("cat".into(), vec![], SpawnConfig::default())
        .expect("spawn cat");
    // `cat` echoes its input, so the marker lands on the rendered screen.
    term.write("AGG-MARKER\n").expect("write");

    let path = std::env::temp_dir().join(format!("ix-dash-it-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let mut publisher = publish(&manager, path.clone(), Duration::from_millis(40))
            .await
            .expect("publish");
        assert_eq!(publisher.path(), path);

        let stream = UnixStream::connect(&path).await.expect("connect");
        let mut lines = BufReader::new(stream).lines();

        let terminal_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .expect("read line")
                    .expect("producer closed the stream");
                if line.is_empty() {
                    continue;
                }
                let snapshot: ProducerSnapshot =
                    serde_json::from_str(&line).expect("parse snapshot");
                assert!(
                    snapshot
                        .producer
                        .starts_with(&format!("{}-", std::process::id())),
                    "producer id should carry this pid: {}",
                    snapshot.producer
                );
                if let Some((id, view)) = first_terminal(&snapshot) {
                    assert_eq!(view.command, "cat");
                    if view.screen.contains("AGG-MARKER") {
                        return id;
                    }
                }
            }
        })
        .await
        .expect("timed out waiting for the marker to appear on the streamed screen");
        assert!(!terminal_id.is_empty());

        // Stopping the publisher unlinks the socket so the aggregator stops
        // listing a dead producer.
        publisher.stop().await;
        assert!(!path.exists(), "stop() should remove the socket file");
    });
}

/// A child that emits a colored run and a `DECSCUSR` bar reaches the reader with
/// SGR-encoded screen bytes and a `cursor_shape` of `"bar"`. This crosses the
/// real boundary: byte stream -> actor sniff + vt100 -> pane -> JSON wire, the
/// same path the dashboard's browser parser consumes.
#[test]
fn producer_carries_sgr_and_cursor_shape() {
    let manager = Arc::new(TuiManager::new());
    let term = manager
        .spawn(
            "printf".into(),
            // Bold-red "hi", reset, then the bar-cursor DECSCUSR (CSI 6 SP q).
            vec![r"\033[1;31mhi\033[0m\033[6 q".into()],
            SpawnConfig::default(),
        )
        .expect("spawn printf");
    drop(term);

    let path = std::env::temp_dir().join(format!("ix-dash-sgr-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let mut publisher = publish(&manager, path.clone(), Duration::from_millis(40))
            .await
            .expect("publish");

        let stream = UnixStream::connect(&path).await.expect("connect");
        let mut lines = BufReader::new(stream).lines();

        let view = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .expect("read line")
                    .expect("producer closed the stream");
                if line.is_empty() {
                    continue;
                }
                let snapshot: ProducerSnapshot =
                    serde_json::from_str(&line).expect("parse snapshot");
                if let Some((_, view)) = first_terminal(&snapshot)
                    && view.cursor_shape == "bar"
                    && view.screen.contains("hi")
                {
                    return view;
                }
            }
        })
        .await
        .expect("timed out waiting for the SGR + bar cursor pane");

        // The screen carries an SGR escape (ESC = 0x1b), not just plain text.
        assert!(
            view.screen.contains('\u{1b}'),
            "screen should carry an SGR escape, got {:?}",
            view.screen
        );
        // bold (1) and red foreground (31) both ride in the run.
        assert!(
            view.screen.contains("1;31m") || view.screen.contains("31"),
            "screen should encode the red foreground, got {:?}",
            view.screen
        );
        assert_eq!(view.cursor_shape, "bar");
        // The cursor lands just past "hi" on the first row.
        assert_eq!(view.cursor_row, 0);
        assert_eq!(view.cursor_col, 2);

        publisher.stop().await;
    });
}
