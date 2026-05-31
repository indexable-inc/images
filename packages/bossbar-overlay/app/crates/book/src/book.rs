//! The typed book domain: a titled, paginated book whose text comes off SQLite
//! rows. Strings are untrusted, but a book carries no enums to parse; the only
//! invariant enforced here is that an empty book still has one (blank) page so the
//! overlay always has a spread to draw.

use glam::DVec2;

/// One book, ready to render: its ordered pages and pinned position.
#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    /// Page bodies in reading order. Newlines separate paragraphs; the renderer
    /// wraps long lines to the page width. Always at least one entry.
    pub pages: Vec<String>,
    /// Pinned on-screen location in logical screen points (top-left of the
    /// spread), set once the book is dragged. `None` keeps it centered. Persisted
    /// to the `x`/`y` columns.
    pub pos: Option<DVec2>,
}

impl Book {
    /// Number of pages, never zero.
    pub fn page_count(&self) -> usize {
        self.pages.len().max(1)
    }

    /// The body of page `i`, or empty past the end.
    pub fn page(&self, i: usize) -> &str {
        self.pages.get(i).map(String::as_str).unwrap_or("")
    }

    /// Highest valid left-page index of a two-page spread (always even). Turning
    /// past it is clamped here so the overlay never shows a blank spread.
    pub fn last_spread(&self) -> usize {
        let last = self.page_count().saturating_sub(1);
        last - (last % 2)
    }
}
