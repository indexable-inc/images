//! End-to-end return-channel test: a browser-shaped `send` input routed by an
//! aggregator reaches the PTY behind the pane, exactly once per send id.
//!
//! Compiled only with the `dashboard` + `publish` features, the same pair the
//! aggregator builds with (and the pair `tui-py` enables, so this matches the
//! unit graph CI compiles). Without them the file is empty.
//!
//! Same runtime shape as `aggregate.rs`: the manager's blocking spawn runs
//! outside any runtime, the producer/consumer halves inside an explicit one,
//! because `TuiManager` owns its own runtime and a `#[tokio::test]` would
//! panic on its blocking calls.
#![cfg(all(feature = "dashboard", feature = "publish"))]

use std::sync::Arc;
use std::time::Duration;

use dashboard_core::{Input, InputLine, ProducerEvent, subscribe_bidi};
use tokio::sync::mpsc;
use tui::{SpawnConfig, TuiManager, View, publish};

/// A browser-shaped send: the LWW `send` choice holding `{id, text}` JSON.
fn send_line(pane: &str, id: &str, text: &str) -> InputLine {
    InputLine {
        pane: pane.to_owned(),
        field: "send".to_owned(),
        value: Input::Choice {
            value: format!(r#"{{"id":"{id}","text":"{text}"}}"#),
        },
    }
}

/// How many times `needle` is on the rendered screen right now.
async fn occurrences(term: &tui::TuiInstance, needle: &str) -> usize {
    term.read_viewport_async()
        .await
        .map_or(0, |lines| lines.join("\n").matches(needle).count())
}

/// Wait until `needle` appears at least `want` times (typed echo + `cat`'s
/// output line), or panic with the screen after a 10s deadline.
async fn wait_for_occurrences(term: &tui::TuiInstance, needle: &str, want: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if occurrences(term, needle).await >= want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{needle:?} never reached {want} occurrences; screen:\n{}",
            term.read_viewport_async()
                .await
                .unwrap_or_default()
                .join("\n"),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn a_routed_send_reaches_the_pty_once_per_id() {
    let manager = Arc::new(TuiManager::new());
    // `cat` echoes typed input and prints each submitted line back, so one
    // delivered send shows its text twice on screen.
    let term = manager
        .spawn("cat".into(), vec![], SpawnConfig::default())
        .expect("spawn cat");
    let pane = term.id.to_string();

    let dir = std::env::temp_dir().join(format!("ix-dash-send-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("p.sock");
    let _ = std::fs::remove_file(&path);

    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let publisher = publish(&manager, path.clone(), Duration::from_millis(40))
            .await
            .expect("publish");
        let producer = publisher.producer_id().to_owned();

        // The aggregator's role: discover the socket, then type back.
        let mut feed = subscribe_bidi(
            dir.clone(),
            Duration::from_millis(20),
            &tokio::runtime::Handle::current(),
        );
        loop {
            match feed.events.recv().await.expect("event") {
                ProducerEvent::Snapshot(snapshot) if !snapshot.panes.is_empty() => break,
                _ => {}
            }
        }

        assert!(
            feed.inputs
                .route(&producer, send_line(&pane, "u1", "SEND-ONE")),
            "the connected producer must accept the send"
        );
        wait_for_occurrences(&term, "SEND-ONE", 2).await;
        let after_first = occurrences(&term, "SEND-ONE").await;

        // A replayed duplicate (same id) must not resubmit; a fresh id must.
        // FIFO on the return channel means that once SEND-TWO is visible, the
        // duplicate before it has already been consumed and skipped.
        assert!(
            feed.inputs
                .route(&producer, send_line(&pane, "u1", "SEND-ONE"))
        );
        assert!(
            feed.inputs
                .route(&producer, send_line(&pane, "u2", "SEND-TWO"))
        );
        wait_for_occurrences(&term, "SEND-TWO", 2).await;
        assert_eq!(
            occurrences(&term, "SEND-ONE").await,
            after_first,
            "a replayed send id must be delivered exactly once"
        );
    });

    let _ = std::fs::remove_dir_all(&dir);
}

/// The first `sends` outcome the producer publishes for `pane`, waiting for
/// the snapshot that carries it.
///
/// A `sends` pane exists only once a send has resolved, so its arrival is the
/// signal; the deadline is generous because an unconfirmed send only resolves
/// after the producer's five-second echo wait plus the two-second turn wait.
async fn first_outcome(
    events: &mut mpsc::Receiver<ProducerEvent>,
    pane: &str,
) -> serde_json::Value {
    let wanted = format!("{pane}-sends");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| panic!("no {wanted} pane before the deadline"))
            .expect("the producer feed stayed open");
        if let ProducerEvent::Snapshot(snapshot) = event {
            for card in snapshot.panes {
                if card.id != wanted {
                    continue;
                }
                let View::Data(data) = card.view else {
                    panic!("the sends pane is a data pane");
                };
                let sends = data.data.get("sends").expect("a sends key").clone();
                let first = sends
                    .as_array()
                    .and_then(|list| list.first())
                    .expect("at least one recorded outcome");
                return first.clone();
            }
        }
    }
}

/// A send the terminal never echoes is published as `unconfirmed`, and one it
/// does echo as `landed`, so the two are distinguishable from the browser.
///
/// The ENG-12530 regression. Delivery typed the text, waited for an echo that
/// never came, pressed Enter and returned; the browser had written a message
/// that reached nothing, and no pane, scrollback line or log said so. The
/// echo-off `cat` here stands in for the observed case, an agent parked on
/// Claude Code's "Loading development channels ... Enter to confirm" gate:
/// the tty accepts every keystroke and shows none of them.
#[test]
fn an_unechoed_send_is_published_as_unconfirmed() {
    let manager = Arc::new(TuiManager::new());
    let quiet = manager
        .spawn(
            "sh".into(),
            vec!["-c".into(), "stty -echo; cat >/dev/null".into()],
            SpawnConfig::default(),
        )
        .expect("spawn the echo-less terminal");
    let loud = manager
        .spawn("cat".into(), vec![], SpawnConfig::default())
        .expect("spawn cat");
    let quiet_pane = quiet.id.to_string();
    let loud_pane = loud.id.to_string();

    let dir = std::env::temp_dir().join(format!("ix-dash-unechoed-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("p.sock");
    let _ = std::fs::remove_file(&path);

    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    runtime.block_on(async {
        let publisher = publish(&manager, path.clone(), Duration::from_millis(40))
            .await
            .expect("publish");
        let producer = publisher.producer_id().to_owned();
        let mut feed = subscribe_bidi(
            dir.clone(),
            Duration::from_millis(20),
            &tokio::runtime::Handle::current(),
        );
        loop {
            match feed.events.recv().await.expect("event") {
                ProducerEvent::Snapshot(snapshot) if !snapshot.panes.is_empty() => break,
                _ => {}
            }
        }

        assert!(
            feed.inputs
                .route(&producer, send_line(&quiet_pane, "q1", "SWALLOWED"))
        );
        let outcome = first_outcome(&mut feed.events, &quiet_pane).await;
        assert_eq!(
            outcome.get("id").and_then(serde_json::Value::as_str),
            Some("q1"),
            "the outcome is keyed by the id the browser stamped, so it can match it"
        );
        assert_eq!(
            outcome.get("state").and_then(serde_json::Value::as_str),
            Some("unconfirmed"),
            "a send the terminal never echoed must not read as delivered; got {outcome}"
        );
        assert!(
            outcome
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|detail| !detail.is_empty()),
            "an unconfirmed send says what was seen instead; got {outcome}"
        );

        assert!(
            feed.inputs
                .route(&producer, send_line(&loud_pane, "l1", "ARRIVED"))
        );
        let landed = first_outcome(&mut feed.events, &loud_pane).await;
        assert_eq!(
            landed.get("state").and_then(serde_json::Value::as_str),
            Some("landed"),
            "a send the terminal echoed is the other outcome; got {landed}"
        );
    });

    let _ = std::fs::remove_dir_all(&dir);
}
