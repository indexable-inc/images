// SQL mirror of reviewer-note annotations stored in the Loro doc.
//
// The Loro doc is the source of truth: each message gets a root
// LoroMap named `annotations:<message_id>` whose entries are
// `{annotation_id => JSON({author_id, author_name, ts_ms, text})}`
// (see `Annotation` / `annotationsFor` in
// `packages/room/src/lib/loro.ts`). This module materializes that
// shape into the `message_annotations` SQL table so retroactive
// AGENTS.md mining can `SELECT … JOIN messages` without rehydrating
// the CRDT.
//
// The mirror runs after every accepted Loro frame and at server
// boot once the persisted log has been replayed. To avoid hammering
// SQL with a delete+insert on every presence heartbeat, an in-
// memory cache keyed by message id remembers the last-mirrored
// annotation set; only message ids whose set actually changed get
// re-written.

use std::collections::HashMap;

use anyhow::{Context, Result};
use loro::{LoroDoc, LoroValue};
use serde::Deserialize;

use crate::db::{Annotation, Db};

/// Container-name prefix that identifies a per-message annotation
/// map. The suffix after the colon is the message id.
const PREFIX: &str = "annotations:";

/// Last-mirrored snapshot, keyed by message id. Compared against the
/// freshly-extracted state on each pass so we only touch SQL for
/// message ids whose annotation set actually changed.
#[derive(Debug, Default)]
pub struct AnnotationMirror {
    cache: HashMap<String, Vec<Annotation>>,
}

impl AnnotationMirror {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk the doc, extract every `annotations:*` map, and reconcile
    /// SQL + cache. Cheap on a steady-state doc — only message ids
    /// whose entries differ from the cache hit the database.
    pub fn sync(&mut self, doc: &LoroDoc, db: &mut Db) -> Result<()> {
        let fresh = extract(doc);

        for (message_id, annotations) in &fresh {
            let prior = self.cache.get(message_id);
            if prior.map(|p| p == annotations).unwrap_or(false) {
                continue;
            }
            db.reconcile_annotations_for(message_id, annotations)
                .with_context(|| format!("reconcile annotations for {message_id}"))?;
        }

        let dropped: Vec<String> = self
            .cache
            .keys()
            .filter(|k| !fresh.contains_key(*k))
            .cloned()
            .collect();
        for message_id in &dropped {
            db.reconcile_annotations_for(message_id, &[])
                .with_context(|| format!("clear annotations for {message_id}"))?;
        }

        self.cache = fresh;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct WireAnnotation {
    author_id: String,
    author_name: String,
    ts_ms: i64,
    text: String,
}

fn extract(doc: &LoroDoc) -> HashMap<String, Vec<Annotation>> {
    let mut out: HashMap<String, Vec<Annotation>> = HashMap::new();
    let LoroValue::Map(root) = doc.get_deep_value() else {
        return out;
    };
    for (key, value) in root.iter() {
        let Some(message_id) = key.strip_prefix(PREFIX) else {
            continue;
        };
        let LoroValue::Map(map) = value else {
            continue;
        };
        let mut entries: Vec<Annotation> = Vec::with_capacity(map.len());
        for (annotation_id, encoded) in map.iter() {
            let LoroValue::String(encoded) = encoded else {
                continue;
            };
            // Wire shape is a JSON-encoded string so the same blob
            // can ride through both LoroMap values and HTTP without
            // an intermediate representation.
            let Ok(parsed) = serde_json::from_str::<WireAnnotation>(encoded.as_str()) else {
                continue;
            };
            entries.push(Annotation {
                id: annotation_id.to_owned(),
                message_id: message_id.to_owned(),
                author_id: parsed.author_id,
                author_name: parsed.author_name,
                ts_ms: parsed.ts_ms,
                text: parsed.text,
            });
        }
        entries.sort_by_key(|a| a.ts_ms);
        out.insert(message_id.to_owned(), entries);
    }
    out
}
