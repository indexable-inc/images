//! The hub: a Loro CRDT document that stores every producer's resources under
//! its own scope and retains the edit history so browsers can time-travel.
//!
//! Each producer gets a Loro map keyed by its [`ProducerId`]. Inside that map,
//! one entry per [`ResourceId`] holds the MessagePack-encoded [`Resource`].
//! Encoding the typed sum keeps the document shape uniform across kinds while
//! the typed boundary lives at the Rust API. The hub never trims history, so
//! past resource states remain reachable through the document's version graph.

use loro::{LoroDoc, LoroValue, ValueOrContainer};
use snafu::{ResultExt, Snafu};

use crate::render::{RenderedResource, render};
use crate::resource::{Resource, ResourceId};

/// Identifier for one producer's scope inside the hub document.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProducerId(String);

impl ProducerId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A producer's latest view of every resource it owns.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ProducerSnapshot {
    pub resources: Vec<Resource>,
}

/// Failures applying or reading a producer snapshot against the hub document.
#[derive(Debug, Snafu)]
pub enum SnapshotError {
    #[snafu(display("failed to encode resource `{id}` for the hub document"))]
    Encode {
        id: ResourceId,
        source: rmp_serde::encode::Error,
    },
    #[snafu(display("failed to apply resource `{id}` to the hub document"))]
    Apply {
        id: ResourceId,
        source: loro::LoroError,
    },
    #[snafu(display("failed to decode the resource at key `{key}` from the hub document"))]
    Decode {
        key: String,
        source: rmp_serde::decode::Error,
    },
}

/// The retained-history store backing the dashboard.
pub struct Hub {
    doc: LoroDoc,
}

impl Hub {
    #[must_use]
    pub fn new() -> Self {
        Self {
            doc: LoroDoc::new(),
        }
    }

    /// Apply a producer's snapshot under its own scope and commit it as one
    /// change, so the document's history gains a single time-travel point per
    /// snapshot.
    pub fn apply_snapshot(
        &self,
        producer: &ProducerId,
        snapshot: &ProducerSnapshot,
    ) -> Result<(), SnapshotError> {
        let scope = self.doc.get_map(producer.as_str());
        for resource in &snapshot.resources {
            let id = resource.id();
            let bytes = rmp_serde::to_vec(resource).context(EncodeSnafu { id: id.clone() })?;
            scope
                .insert(id.as_str(), bytes)
                .context(ApplySnafu { id: id.clone() })?;
        }
        self.doc.commit();
        Ok(())
    }

    /// Decode every resource currently stored under a producer's scope, ordered
    /// by resource key for a stable render.
    pub fn resources(&self, producer: &ProducerId) -> Result<Vec<Resource>, SnapshotError> {
        let scope = self.doc.get_map(producer.as_str());
        let mut keys = scope.keys().map(|key| key.to_string()).collect::<Vec<_>>();
        keys.sort();

        let mut resources = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(ValueOrContainer::Value(LoroValue::Binary(bytes))) = scope.get(&key) else {
                continue;
            };
            let resource = rmp_serde::from_slice(bytes.as_slice()).context(DecodeSnafu { key })?;
            resources.push(resource);
        }
        Ok(resources)
    }

    /// Render every resource under a producer's scope for the dashboard shell.
    pub fn render(&self, producer: &ProducerId) -> Result<Vec<RenderedResource>, SnapshotError> {
        Ok(self.resources(producer)?.iter().map(render).collect())
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{Hub, ProducerId, ProducerSnapshot};
    use crate::resource::{
        BrowserPage, ImageMediaType, Resource, ResourceId, ResourceKind, Screenshot, TerminalFrame,
        TerminalSize,
    };

    #[test]
    fn round_trips_mixed_resources_through_loro_and_renders_each_kind() {
        let hub = Hub::new();
        let producer = ProducerId::new("agent-0");

        let terminal = Resource::Terminal(TerminalFrame {
            id: ResourceId::new("term-1"),
            title: "shell".to_owned(),
            size: TerminalSize { cols: 80, rows: 24 },
            rows: vec!["$ echo hi".to_owned(), "hi".to_owned()],
        });
        let page = Resource::BrowserPage(BrowserPage {
            id: ResourceId::new("page-1"),
            url: Url::parse("https://example.com/docs").expect("valid url"),
            screenshot: Screenshot {
                media_type: ImageMediaType::Png,
                bytes: vec![1, 2, 3, 4],
            },
            dom_text: Some("Example".to_owned()),
        });

        let snapshot = ProducerSnapshot {
            resources: vec![terminal.clone(), page.clone()],
        };
        hub.apply_snapshot(&producer, &snapshot).expect("apply");

        // Crossing the Loro boundary and decoding back must preserve both kinds.
        let stored = hub.resources(&producer).expect("read");
        assert_eq!(stored.len(), 2);
        assert!(stored.contains(&terminal));
        assert!(stored.contains(&page));

        let rendered = hub.render(&producer).expect("render");
        let kinds = rendered.iter().map(|r| r.kind).collect::<Vec<_>>();
        assert!(kinds.contains(&ResourceKind::Terminal));
        assert!(kinds.contains(&ResourceKind::BrowserPage));

        let page_fragment = rendered
            .iter()
            .find(|r| r.kind == ResourceKind::BrowserPage)
            .expect("browser page fragment");
        assert!(page_fragment.html.contains("data:image/png;base64,"));
        assert!(page_fragment.html.contains("https://example.com/docs"));
    }
}
