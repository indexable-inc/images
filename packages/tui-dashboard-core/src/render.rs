//! Per-kind rendering of a [`Resource`] into a self-contained HTML fragment.
//!
//! The dashboard shell slots each fragment into a container keyed by
//! [`ResourceId`], so a new browser page auto-appears the same way a new
//! terminal does today. [`render`] matches every kind exhaustively: a future
//! variant cannot be added without giving it a fragment.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::resource::{BrowserPage, Resource, ResourceId, ResourceKind, TerminalFrame};

/// A resource rendered to an HTML fragment plus the metadata the shell needs
/// to place it and update it in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedResource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub title: String,
    pub html: String,
}

/// Render one resource for the dashboard.
#[must_use]
pub fn render(resource: &Resource) -> RenderedResource {
    match resource {
        Resource::Terminal(frame) => render_terminal(frame),
        Resource::BrowserPage(page) => render_browser_page(page),
    }
}

fn render_terminal(frame: &TerminalFrame) -> RenderedResource {
    let body = frame
        .rows
        .iter()
        .map(|row| escape_html(row))
        .collect::<Vec<_>>()
        .join("\n");
    let html = format!(
        "<pre class=\"resource resource--{slug}\" data-resource-id=\"{id}\">{body}</pre>",
        slug = ResourceKind::Terminal.slug(),
        id = escape_html(frame.id.as_str()),
    );
    RenderedResource {
        id: frame.id.clone(),
        kind: ResourceKind::Terminal,
        title: frame.title.clone(),
        html,
    }
}

fn render_browser_page(page: &BrowserPage) -> RenderedResource {
    let data_url = format!(
        "data:{};base64,{}",
        page.screenshot.media_type.as_mime(),
        STANDARD.encode(&page.screenshot.bytes),
    );
    let href = escape_html(page.url.as_str());
    let caption = page
        .dom_text
        .as_deref()
        .map(escape_html)
        .map_or_else(String::new, |text| {
            format!("<figcaption class=\"resource__text\">{text}</figcaption>")
        });
    let html = format!(
        "<figure class=\"resource resource--{slug}\" data-resource-id=\"{id}\">\
<img src=\"{data_url}\" alt=\"Screenshot of {href}\">\
<a class=\"resource__url\" href=\"{href}\">{href}</a>\
{caption}</figure>",
        slug = ResourceKind::BrowserPage.slug(),
        id = escape_html(page.id.as_str()),
    );
    RenderedResource {
        id: page.id.clone(),
        kind: ResourceKind::BrowserPage,
        title: page.url.host_str().unwrap_or(page.url.as_str()).to_owned(),
        html,
    }
}

/// Escape the five characters that change meaning inside HTML text and
/// double-quoted attribute values. Producer-supplied titles, URLs, and DOM
/// text are untrusted, so every interpolated string passes through here.
fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}
