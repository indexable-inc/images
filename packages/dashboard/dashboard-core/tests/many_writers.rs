//! Fifteen agents on one document: the fan-out the design exists for.
//!
//! Two agents appending is the mechanism; fifteen is the load, and it is where
//! the self-wake bug stops being a wasted tick and becomes a storm. If a delta
//! named every peer in the version vector rather than the peers that moved, then
//! with N writers every write would wake all N including its author, so one
//! round of writes costs N^2 wakes instead of N.
//!
//! These are deterministic and link-independent: authorship is computed at
//! commit time from the version vector, so latency cannot change the answer.
//! What a real link adds is tested elsewhere.

use std::collections::HashSet;

use dashboard_core::{Hub, Pane};
use loro::{LoroDoc, PeerID, ToJson as _};

const WRITERS: usize = 15;

/// Peer ids the hub hands out, one per agent. Deliberately not derived from the
/// agent's name: Loro's guidance is that a fixed id per user is unsafe, and the
/// safety here comes from the ids never leaving the hub's lock.
fn peer_of(agent: usize) -> PeerID {
    1000 + agent as PeerID
}

fn pane_for(agent: usize, tick: usize) -> Pane {
    Pane::html(
        format!("finding-{agent}"),
        format!("agent {agent}"),
        format!("<p>measurement {tick} from agent {agent}</p>"),
    )
}

/// Every write names exactly its own author, never a peer that merely exists.
///
/// This is the storm guard at fan-out. `update_authors.rs` proves the property
/// for one writer plus one remote; this proves it does not decay as writers
/// accumulate, which is the case that actually bites.
#[test]
fn fifteen_writers_each_wake_only_for_their_own_write() {
    let hub = Hub::new();
    let mut updates = hub.updates();

    // Round one: every agent introduces a finding. Each is one scope so agents
    // do not reconcile each other away.
    for agent in 0..WRITERS {
        hub.apply_scope(&format!("agent-{agent}"), &[pane_for(agent, 0)]);
    }

    let mut seen: Vec<Vec<PeerID>> = Vec::new();
    while let Ok(update) = updates.try_recv() {
        seen.push(update.authors().to_vec());
    }

    assert_eq!(seen.len(), WRITERS, "one delta per writer");
    for (index, authors) in seen.iter().enumerate() {
        assert_eq!(
            authors.len(),
            1,
            "delta {index} must name exactly one author, named {authors:?}"
        );
    }

    // Every delta is the hub's own peer here, because `apply_scope` does not yet
    // take a writer identity -- that is the `set_pane` work. The property under
    // test is the shape: one author per delta, never the accumulated set.
    let distinct: HashSet<PeerID> = seen.iter().flatten().copied().collect();
    assert_eq!(distinct.len(), 1, "one writing peer so far: {distinct:?}");
}

/// The same fan-out with a genuine peer per agent, which is where a delta naming
/// the whole version vector would show up as an N-times-too-large author set.
#[test]
fn a_peer_per_agent_keeps_author_sets_singular_as_peers_accumulate() {
    let doc = LoroDoc::new();
    let body = doc.get_text("body");
    let mut streamed = doc.oplog_vv();

    for agent in 0..WRITERS {
        doc.set_peer_id(peer_of(agent)).expect("set_peer_id");
        body.insert(body.len_unicode(), &format!("{agent} "))
            .expect("insert");
        doc.commit();

        let current = doc.oplog_vv();
        let moved: Vec<PeerID> = current
            .iter()
            .filter(|(peer, counter)| {
                streamed.get(peer).copied().unwrap_or_default() < **counter
            })
            .map(|(peer, _)| *peer)
            .collect();

        assert_eq!(
            moved,
            vec![peer_of(agent)],
            "after {} peers exist, agent {agent}'s write must still name only itself",
            agent + 1
        );
        streamed = current;
    }

    // All fifteen are in the document, and each character knows its author.
    assert_eq!(doc.oplog_vv().len(), WRITERS, "every agent has its own line");
    assert_eq!(body.get_editor_at_unicode_pos(0), Some(peer_of(0)));
    assert_eq!(body.get_editor_at_unicode_pos(2), Some(peer_of(1)));
}

/// Fifteen agents writing into one shared document converge, and every agent's
/// contribution survives -- nobody's pane is reconciled away by a neighbour.
#[test]
fn fifteen_writers_all_survive_in_the_document() {
    let hub = Hub::new();
    for tick in 0..3 {
        for agent in 0..WRITERS {
            hub.apply_scope(&format!("agent-{agent}"), &[pane_for(agent, tick)]);
        }
    }

    let replica = LoroDoc::new();
    replica.import(&hub.export_snapshot()).expect("snapshot");
    let json = replica.get_deep_value().to_json();

    for agent in 0..WRITERS {
        assert!(
            json.contains(&format!("measurement 2 from agent {agent}")),
            "agent {agent}'s latest finding must be in the document"
        );
    }
}
