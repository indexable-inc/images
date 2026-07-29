//! The author signal on a broadcast update (ENG-10199).
//!
//! An in-process consumer that both writes to a hub and watches it needs to tell
//! its own writes from everyone else's, or it wakes itself on every tick. Before
//! this signal existed the only public read was a whole-document snapshot, which
//! says nothing about who wrote anything, so `packages/dashboard/ex` polled it on
//! a timer and filtered by root container as a proxy for "was this a human".

use std::sync::Arc;

use dashboard_core::{Hub, Pane};
use loro::{ExportMode, LoroDoc};

fn html(id: &str, body: &str) -> Pane {
    Pane::html(id, "t", body.to_owned())
}

/// A local write is attributed to the hub's own peer, so a consumer acting for
/// the hub can recognise its own echo and skip it.
#[test]
fn a_local_write_is_attributed_to_the_hubs_peer() {
    let hub = Hub::new();
    let mut updates = hub.updates();

    hub.apply_scope("agent", &[html("a", "one")]);

    let update = updates.try_recv().expect("a write must broadcast");
    assert_eq!(update.authors(), [hub.peer_id()], "the hub wrote this");
    assert!(update.is_only_from(hub.peer_id()), "and only the hub");
    assert!(!update.bytes().is_empty(), "the delta carries the ops");
}

/// A merge from a browser is attributed to the browser's peer, never the hub's.
/// This is the case the old root-container filter stood in for.
#[test]
fn an_imported_edit_is_attributed_to_the_remote_peer() {
    let hub = Hub::new();
    hub.apply_scope("agent", &[html("a", "one")]);

    // A browser: its own document and peer, seeded from the hub.
    let browser = LoroDoc::new();
    browser.import(&hub.export_snapshot()).expect("seed");
    let browser_peer = browser.peer_id();
    assert_ne!(browser_peer, hub.peer_id());

    // It answers an input, which is the surface no producer writes.
    let inputs = browser.get_map("inputs");
    inputs.insert("a\u{1f}answer", "yes").expect("insert");
    browser.commit();

    let mut updates = hub.updates();
    hub.import(
        &browser
            .export(ExportMode::all_updates())
            .expect("browser updates"),
    )
    .expect("import");

    let update = updates.try_recv().expect("an import must broadcast");
    assert!(
        update.authors().contains(&browser_peer),
        "the browser's ops are in this delta: {:?}",
        update.authors()
    );
    assert!(
        !update.is_only_from(hub.peer_id()),
        "an import is not the hub's own echo"
    );
}

/// The one that matters for self-wake: once a browser peer exists in the
/// document, a later *local* write must still name only the hub.
///
/// The author set is the peers whose counter moved since the last broadcast, not
/// every peer in the version vector. Getting that wrong is invisible until a
/// second peer joins, and then a consumer filtering "was this someone other than
/// me" wakes on its own every write, forever. That is the ENG-10199 storm.
#[test]
fn a_local_write_after_a_remote_join_names_only_the_hub() {
    let hub = Hub::new();
    hub.apply_scope("agent", &[html("a", "one")]);

    let browser = LoroDoc::new();
    browser.import(&hub.export_snapshot()).expect("seed");
    let browser_peer = browser.peer_id();
    let inputs = browser.get_map("inputs");
    inputs.insert("a\u{1f}answer", "yes").expect("insert");
    browser.commit();
    hub.import(
        &browser
            .export(ExportMode::all_updates())
            .expect("browser updates"),
    )
    .expect("import");

    // Both peers are now in the hub's version vector. A local write must still
    // be attributed to the hub alone.
    let mut updates = hub.updates();
    hub.apply_scope("agent", &[html("a", "two")]);

    let update = updates.try_recv().expect("the local write must broadcast");
    assert_eq!(
        update.authors(),
        [hub.peer_id()],
        "only the hub moved; the browser's earlier ops must not be re-reported"
    );
    assert!(
        !update.authors().contains(&browser_peer),
        "a settled remote peer is not an author of someone else's later write"
    );
    assert!(
        update.is_only_from(hub.peer_id()),
        "so a consumer acting for the hub recognises this as its own echo"
    );
}

/// The subscription's snapshot and stream are taken under one lock, so a
/// consumer that seeds from the snapshot and then applies every update ends up
/// byte-identical to the hub with no gap and no double-apply.
#[test]
fn seeding_from_a_subscription_then_applying_updates_converges() {
    let hub = Hub::new();
    hub.apply_scope("agent", &[html("a", "before")]);

    let (snapshot, mut updates) = hub.subscribe().into_parts();
    let replica = LoroDoc::new();
    replica.import(&snapshot).expect("seed from snapshot");

    // Writes after the subscription arrive on the stream.
    hub.apply_scope("agent", &[html("a", "before"), html("b", "after")]);
    hub.apply_scope("agent", &[html("a", "changed"), html("b", "after")]);

    let mut applied = 0;
    while let Ok(update) = updates.try_recv() {
        replica.import(update.bytes()).expect("apply update");
        applied += 1;
    }
    assert_eq!(applied, 2, "both writes reached the stream");

    let hub_doc = LoroDoc::new();
    hub_doc.import(&hub.export_snapshot()).expect("hub snapshot");
    assert_eq!(
        replica.get_deep_value(),
        hub_doc.get_deep_value(),
        "replica must match the hub exactly"
    );
}

/// A write that changes nothing broadcasts nothing, so a consumer is not woken
/// by an idle producer re-publishing the same panes.
#[test]
fn an_unchanged_rewrite_broadcasts_nothing() {
    let hub = Hub::new();
    hub.apply_scope("agent", &[html("a", "one")]);

    let mut updates = hub.updates();
    hub.apply_scope("agent", &[html("a", "one")]);

    assert!(
        updates.try_recv().is_err(),
        "republishing identical panes must not wake anyone"
    );
}

/// Several subscribers each see every update: the signal fans out, so one
/// consumer taking it does not starve another.
#[test]
fn every_subscriber_sees_every_update() {
    let hub = Hub::new();
    let mut first = hub.updates();
    let mut second = hub.updates();

    hub.apply_scope("agent", &[html("a", "one")]);

    let a = first.try_recv().expect("first subscriber");
    let b = second.try_recv().expect("second subscriber");
    assert!(Arc::ptr_eq(&a, &b), "one Arc shared, encoded once at fan-in");
}
