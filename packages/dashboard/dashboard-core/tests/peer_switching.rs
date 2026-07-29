//! Proof for the one risky call in the multi-agent attribution design: the hub
//! switches its document's peer id per writing agent, under the lock it already
//! holds, so every agent's ops carry that agent's peer.
//!
//! Loro's own guidance is to leave peer ids random and never pin one per user,
//! *unless* you have strict single-ownership locking. This hub is that case: one
//! `LoroDoc` behind one mutex, ids that never leave it, so two writers can never
//! produce ops under the same id concurrently. These tests hold that claim to
//! the behaviour rather than to the reasoning.

use std::sync::Arc;

use loro::{LoroDoc, ToJson as _};
use parking_lot::Mutex;

/// Peers this "hub" hands out, standing in for the `__peers` registry lookup.
const ALICE: u64 = 11;
const BOB: u64 = 22;

/// Write `text` at the end of the `body` text container as `peer`.
fn write_as(doc: &LoroDoc, peer: u64, text: &str) {
    doc.set_peer_id(peer).expect("set_peer_id");
    let body = doc.get_text("body");
    body.insert(body.len_unicode(), text).expect("insert");
    doc.commit();
}

/// Switching peers between commits keeps each peer's counter its own, and the
/// version vector ends up naming every peer that wrote.
#[test]
fn peer_switching_gives_each_writer_its_own_counter_line() {
    let doc = LoroDoc::new();

    write_as(&doc, ALICE, "alice one. ");
    write_as(&doc, BOB, "bob one. ");
    write_as(&doc, ALICE, "alice two. ");

    let vv = doc.oplog_vv();
    let alice = vv.get(&ALICE).copied().unwrap_or_default();
    let bob = vv.get(&BOB).copied().unwrap_or_default();

    assert!(alice > 0, "alice must have ops: {vv:?}");
    assert!(bob > 0, "bob must have ops: {vv:?}");
    // Alice wrote 11 + 11 = 22 characters across two turns, Bob 9 in one. The
    // point is not the exact counter but that each peer has its own line and
    // Alice's spans both of her turns rather than restarting.
    assert_eq!(alice, 22, "alice's counter must continue across her two turns");
    assert_eq!(bob, 9, "bob's counter must count only bob's ops");

    assert_eq!(doc.get_text("body").to_string(), "alice one. bob one. alice two. ");
}

/// The whole point of per-agent peers: a character knows which agent typed it.
#[test]
fn get_editor_of_attributes_each_span_to_the_agent_that_wrote_it() {
    let doc = LoroDoc::new();
    write_as(&doc, ALICE, "AAAA");
    write_as(&doc, BOB, "BBBB");

    let body = doc.get_text("body");
    assert_eq!(body.get_editor_at_unicode_pos(0), Some(ALICE), "first char is alice's");
    assert_eq!(body.get_editor_at_unicode_pos(3), Some(ALICE), "last alice char");
    assert_eq!(body.get_editor_at_unicode_pos(4), Some(BOB), "first bob char");
    assert_eq!(body.get_editor_at_unicode_pos(7), Some(BOB), "last bob char");
}

/// A peer-switching document still converges with a genuinely remote peer that
/// edited concurrently. This is the failure the Loro docs warn about, so it is
/// the one worth demonstrating rather than assuming.
#[test]
fn hub_with_switching_peers_converges_with_a_concurrent_remote() {
    let hub = LoroDoc::new();
    write_as(&hub, ALICE, "alice. ");

    // A browser: its own document, its own random peer, seeded from the hub.
    let browser = LoroDoc::new();
    browser
        .import(&hub.export(loro::ExportMode::Snapshot).expect("snapshot"))
        .expect("seed");
    let browser_peer = browser.peer_id();
    assert_ne!(browser_peer, ALICE);
    assert_ne!(browser_peer, BOB);

    // Concurrent edits: the hub writes as Bob while the browser writes as itself.
    write_as(&hub, BOB, "bob. ");
    let body = browser.get_text("body");
    body.insert(body.len_unicode(), "human. ").expect("insert");
    browser.commit();

    // Exchange both ways.
    let from_hub = hub.export(loro::ExportMode::all_updates()).expect("hub updates");
    let from_browser = browser
        .export(loro::ExportMode::all_updates())
        .expect("browser updates");
    browser.import(&from_hub).expect("import hub");
    hub.import(&from_browser).expect("import browser");

    assert_eq!(
        hub.get_text("body").to_string(),
        browser.get_text("body").to_string(),
        "both sides must converge on the same text"
    );
    assert_eq!(
        hub.get_deep_value().to_json(),
        browser.get_deep_value().to_json(),
        "both sides must converge on the same document"
    );

    // And attribution survives the round trip on the side that did not write it.
    let merged = browser.get_text("body");
    assert_eq!(merged.get_editor_at_unicode_pos(0), Some(ALICE));
}

/// Block-level attribution -- "who last set this block's title" -- has its own
/// O(1) primitive, so the cheap tier needs no oplog fold either.
#[test]
fn map_scalars_report_their_last_editor() {
    let doc = LoroDoc::new();

    doc.set_peer_id(ALICE).expect("set_peer_id");
    let meta = doc.get_map("meta");
    meta.insert("title", "alice's finding").expect("insert");
    meta.insert("kind", "exec").expect("insert");
    doc.commit();

    // Bob overwrites the title. This is the convention violation the design
    // leaves unenforced; the point is that it is now visible on the block.
    doc.set_peer_id(BOB).expect("set_peer_id");
    meta.insert("title", "bob's correction").expect("insert");
    doc.commit();

    assert_eq!(meta.get_last_editor("title"), Some(BOB), "title changed hands");
    assert_eq!(meta.get_last_editor("kind"), Some(ALICE), "kind is untouched");
}

/// `subscribe_first_commit_from_peer` is what writes a peer's `__peers` entry.
/// With one peer per agent it has to fire once per agent, not once per document.
#[test]
fn first_commit_callback_fires_once_per_agent_peer() {
    let doc = LoroDoc::new();
    let seen: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));

    let _guard = {
        let seen = Arc::clone(&seen);
        doc.subscribe_first_commit_from_peer(Box::new(move |payload| {
            seen.lock().push(payload.peer);
            true
        }))
    };

    write_as(&doc, ALICE, "a");
    write_as(&doc, BOB, "b");
    // Alice again: she has already introduced herself, so no second callback.
    write_as(&doc, ALICE, "a2");

    let seen = seen.lock().clone();
    assert_eq!(
        seen,
        vec![ALICE, BOB],
        "one callback per peer, in first-write order, and no repeat for alice"
    );
}
