//! End-to-end producer wire test: a real PTY's rendered screen reaches a
//! reader (the aggregator's role) over the unix socket as a `ProducerSnapshot`.
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
use tui::{ProducerSnapshot, SpawnConfig, TuiManager, publish};

#[test]
fn producer_streams_live_terminal_over_socket() {
    let manager = Arc::new(TuiManager::new());
    let term = manager
        .spawn("cat".into(), vec![], SpawnConfig::default())
        .expect("spawn cat");
    // `cat` echoes its input, so the marker lands on the rendered screen.
    term.write("AGG-MARKER\n").expect("write");

    let path = std::env::temp_dir().join(format!("ix-tui-it-{}.sock", std::process::id()));
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
                if let Some(frame) = snapshot.terminals.first() {
                    assert_eq!(frame.command, "cat");
                    if frame.screen.contains("AGG-MARKER") {
                        return frame.id.clone();
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
