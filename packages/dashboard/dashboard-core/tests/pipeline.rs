//! End-to-end pipeline tests for the exec-pane and recording path.
//!
//! These drive the real public surface a producer and the aggregator use, with a
//! real producer socket, the live HTTP server, and a real Loro snapshot decode,
//! so the wire format, the hub fold, the served page, and replay durability are
//! all exercised together rather than in isolation.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashboard_core::{ExecView, Hub, Pane, ProducerSnapshot, Publisher, RecordingStore, serve_hub};
use loro::LoroDoc;

fn exec_pane() -> Pane {
    Pane::exec(
        "call-1",
        ExecView {
            source: "import subprocess\nsubprocess.run(['echo', 'hi'])".to_owned(),
            lang: "python".to_owned(),
            stdout: "hi-from-echo\n".to_owned(),
            stderr: String::new(),
            result: String::new(),
            running: false,
            ok: Some(true),
            duration_ms: Some(7),
            topic: Some("test".to_owned()),
            line: None,
            error_line: None,
            trace: Vec::new(),
        },
    )
}

/// A producer publishes an exec pane over its discovery socket and a reader (the
/// aggregator's role) parses it back intact: the full producer wire round-trip a
/// `python_exec` takes to reach the board.
#[test]
fn producer_socket_streams_exec_pane() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let dir = std::env::temp_dir().join(format!("ix-dash-pipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("producer.sock");

    let publisher = Publisher::bind(path.clone(), runtime.handle()).expect("bind producer");
    publisher.publish(&[exec_pane()]);

    // Connect as the aggregator does and read the one NDJSON snapshot line.
    let mut stream = UnixStream::connect(&path).expect("connect producer");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).is_ok_and(|n| n == 1) {
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
    }
    let line = String::from_utf8(buf).expect("utf8 line");
    let snapshot: ProducerSnapshot = serde_json::from_str(&line).expect("parse snapshot");

    assert_eq!(snapshot.panes.len(), 1);
    assert_eq!(snapshot.panes[0].view.kind(), "exec");
    assert_eq!(snapshot.panes[0].title, "import subprocess");

    drop(publisher);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Folding an exec pane into the hub and exporting the snapshot yields a Loro
/// document that decodes back to the captured output, a stamped `created_at`, and
/// timestamped history, the three things replay relies on.
#[test]
fn hub_snapshot_decodes_exec_pane_with_history() {
    let hub = Hub::new();
    hub.apply_scope("producer-1", &[exec_pane()]);

    let snapshot = hub.export_snapshot();
    assert!(!snapshot.is_empty(), "snapshot must export bytes");

    let doc = LoroDoc::new();
    doc.import(&snapshot).expect("import snapshot");

    // The decoded document carries the exec pane, its captured stdout, the
    // once-stamped creation time, and the source behind it.
    let dump = format!("{:?}", doc.get_deep_value());
    for needle in ["exec", "hi-from-echo", "created_at", "import subprocess"] {
        assert!(
            dump.contains(needle),
            "decoded doc missing {needle:?}: {dump}"
        );
    }

    // The snapshot carries the full oplog, not a shallow/gc'd one, so the browser
    // can check out and replay any past version rather than only the latest state.
    assert!(
        !doc.is_shallow(),
        "snapshot must retain full history for replay"
    );
}

/// A recording saved to disk reloads to a document that still replays the exec
/// pane: history survives a restart, which is what makes a recording shareable.
#[test]
fn recording_round_trips_through_disk() {
    let hub = Hub::new();
    hub.apply_scope("producer-1", &[exec_pane()]);

    let dir = std::env::temp_dir().join(format!("ix-dash-rec-pipe-{}", std::process::id()));
    let store = RecordingStore::new(dir.clone()).expect("store");
    let id = "rec-1700000000000";
    store.save(id, &hub.export_snapshot()).expect("save");

    let bytes = store.load(id).expect("reload recording");
    let doc = LoroDoc::new();
    doc.import(&bytes).expect("import reloaded recording");
    let dump = format!("{:?}", doc.get_deep_value());
    assert!(
        dump.contains("hi-from-echo"),
        "reloaded recording must replay output"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The HTTP surface serves the page and lists a saved recording: a browser can
/// discover and open a recording over the same server that streams live.
#[test]
fn http_serves_page_and_lists_recordings() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = runtime.enter();

    let dir = std::env::temp_dir().join(format!("ix-dash-http-{}", std::process::id()));
    let store = Arc::new(RecordingStore::new(dir.clone()).expect("store"));
    store.save("rec-1700000000000", b"snapshot").expect("save");

    let hub = Hub::new();
    let addr = "127.0.0.1:0".parse().unwrap();
    let served = runtime
        .block_on(serve_hub(hub, addr, Some(store), runtime.handle()))
        .expect("serve");
    let addr = served.dashboard.addr();

    assert!(http_get(addr, "/").contains("ix"), "index page must render");
    let recordings = http_get(addr, "/recordings");
    assert!(
        recordings.contains("rec-1700000000000"),
        "recordings list must include the saved recording: {recordings}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A minimal blocking HTTP/1.1 GET that reads the whole response to EOF
/// (`Connection: close`), returning the raw response text. Enough to assert the
/// served body without pulling in an HTTP client.
fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match TcpStream::connect(addr) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("connect {addr}: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .expect("write request");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}
