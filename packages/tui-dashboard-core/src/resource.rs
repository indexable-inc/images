//! The typed resource sum the dashboard knows how to store and render.
//!
//! Before this milestone the only resource was a terminal frame. The sum sits
//! behind that single kind so producers can publish other MCP resource kinds
//! through one channel. Two kinds are realized here; the RFC leaves room for
//! `Image`, `Table`, `LogStream`, and `Text` kinds. Add each as a new
//! [`Resource`] variant and the renderer's exhaustive match forces a matching
//! per-kind branch rather than a silent fallthrough.

use serde::{Deserialize, Serialize};
use url::Url;

/// Stable identifier for one resource within a single producer's scope.
///
/// Wrapping the string keeps callers from passing an arbitrary label where a
/// resource key is expected, and gives the hub one place to change the key
/// encoding later.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceId(String);

impl ResourceId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One MCP resource published to the dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resource {
    /// A rendered terminal screen: the only kind before this milestone.
    Terminal(TerminalFrame),
    /// A captured browser page with a screenshot and optional extracted text.
    BrowserPage(BrowserPage),
}

impl Resource {
    /// The resource's identifier, regardless of kind.
    #[must_use]
    pub const fn id(&self) -> &ResourceId {
        match self {
            Self::Terminal(frame) => &frame.id,
            Self::BrowserPage(page) => &page.id,
        }
    }

    /// The lightweight discriminant the dashboard uses to slot a resource into
    /// the right per-kind container.
    #[must_use]
    pub const fn kind(&self) -> ResourceKind {
        match self {
            Self::Terminal(_) => ResourceKind::Terminal,
            Self::BrowserPage(_) => ResourceKind::BrowserPage,
        }
    }
}

/// The set of realized resource kinds. New [`Resource`] variants add a member
/// here so the dashboard shell can group and style them without inspecting the
/// full payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    Terminal,
    BrowserPage,
}

impl ResourceKind {
    /// Stable slug used in CSS classes and `data-` attributes.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::BrowserPage => "browser-page",
        }
    }
}

/// A rendered terminal screen decoded to text, one entry per row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalFrame {
    pub id: ResourceId,
    pub title: String,
    pub size: TerminalSize,
    pub rows: Vec<String>,
}

/// Terminal grid dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

/// A captured browser page: where it was, what it looked like, and optionally
/// the extracted text for search and accessibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPage {
    pub id: ResourceId,
    pub url: Url,
    pub screenshot: Screenshot,
    /// Extracted DOM or text dump when the producer captured one.
    pub dom_text: Option<String>,
}

/// Screenshot bytes tagged with their media type so the renderer can emit a
/// correct `data:` URL without sniffing the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screenshot {
    pub media_type: ImageMediaType,
    pub bytes: Vec<u8>,
}

/// Image encodings a producer may attach to a [`BrowserPage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMediaType {
    Png,
    Jpeg,
    Webp,
}

impl ImageMediaType {
    /// The IANA media type string for a `data:` URL or `Content-Type` header.
    #[must_use]
    pub const fn as_mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}
