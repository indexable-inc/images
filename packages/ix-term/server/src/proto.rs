//! The browser wire protocol: JSON text frames over a per-session websocket.
//!
//! The format is deliberately engine- and transport-agnostic (index#3797
//! keeps it swappable for a ghostty-web WASM client speaking raw bytes): the
//! server ships *dirty rows* — for every changed row, the full new row
//! content as styled spans — plus cursor state, and the client patches its
//! DOM grid. Nothing in the frame references libghostty-vt.
//!
//! Serialization is externally observable behavior (the Svelte client and
//! any future client depend on it), so the tests here pin exact JSON bytes.

use serde::{Deserialize, Serialize};

/// One session in the tab bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionMetaWire {
    /// The session id (a UUID; also the directory name under
    /// `/run/ix-term/sessions`).
    pub id: String,
    /// The user-visible tab name.
    pub name: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at_ms: u64,
}

/// One styled run of text within a row. Attribute keys are omitted when
/// false and colors when the terminal default applies, so a plain-text row
/// serializes as just `{"text":"…"}`.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one bool per independent SGR attribute on the wire; they are not a state enum"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Span {
    /// The run's text, one cell per column (spaces for empty cells).
    pub text: String,
    /// Resolved foreground as `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    /// Resolved background as `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    /// Bold (SGR 1).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// Italic (SGR 3).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    /// Any underline (SGR 4 family).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    /// Inverse video (SGR 7); the client swaps its effective fg/bg.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inverse: bool,
    /// Faint (SGR 2).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dim: bool,
    /// Strikethrough (SGR 9).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strikethrough: bool,
}

/// The full new content of one changed row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowUpdate {
    /// Zero-based viewport row.
    pub y: u16,
    /// The row's cells as consecutive styled runs covering every column.
    pub spans: Vec<Span>,
}

/// Cursor state shipped with every grid frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CursorWire {
    /// Zero-based column.
    pub x: u16,
    /// Zero-based viewport row.
    pub y: u16,
    /// Whether the cursor is shown.
    pub visible: bool,
    /// Requested shape.
    pub shape: CursorShapeWire,
}

/// The cursor shape names the client renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShapeWire {
    /// Filled block.
    Block,
    /// Vertical bar.
    Bar,
    /// Underline.
    Underline,
    /// Hollow block (unfocused terminals).
    Hollow,
}

impl From<ix_vt::CursorVisualStyle> for CursorShapeWire {
    fn from(style: ix_vt::CursorVisualStyle) -> Self {
        match style {
            ix_vt::CursorVisualStyle::Bar => Self::Bar,
            ix_vt::CursorVisualStyle::Block => Self::Block,
            ix_vt::CursorVisualStyle::Underline => Self::Underline,
            ix_vt::CursorVisualStyle::BlockHollow => Self::Hollow,
        }
    }
}

/// Server-to-client messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// First message on a terminal socket: the client's connection id and
    /// the session it joined.
    Hello {
        /// This connection's id, compared against [`ServerMsg::Driver`] to
        /// know whether the client holds the driver seat.
        conn: String,
        /// The joined session.
        session: SessionMetaWire,
    },
    /// A grid frame: `full` frames carry every row (and may change the grid
    /// size); patch frames carry only rows that changed.
    Grid {
        /// Monotonic frame number within the session.
        seq: u64,
        /// Grid width in columns.
        cols: u16,
        /// Grid height in rows.
        rows: u16,
        /// Whether `changed` covers the entire grid.
        full: bool,
        /// The dirty rows, each with its full new content.
        changed: Vec<RowUpdate>,
        /// Cursor state, if the cursor is inside the viewport.
        cursor: Option<CursorWire>,
        /// Whether DECCKM (application cursor keys) is set; selects the
        /// arrow-key encoding the client must send.
        app_cursor: bool,
    },
    /// The driver seat changed hands or the authoritative size changed.
    Driver {
        /// The connection holding the seat, or `None` when it is free.
        conn: Option<String>,
        /// Authoritative grid width.
        cols: u16,
        /// Authoritative grid height.
        rows: u16,
    },
    /// The session's opened document changed. `path` of `None` closes the
    /// split view.
    Open {
        /// Absolute path of the opened HTML file.
        path: Option<String>,
        /// Cache-buster for the iframe URL; bumps on every open.
        nonce: u64,
    },
    /// An error to render inside the session view (the issue's "server
    /// renders errors in the session UI" path — there is no backchannel to
    /// the writer).
    OpenError {
        /// Human-readable reason.
        message: String,
    },
    /// The session's child process exited.
    Exit {
        /// The exit code, if the process exited normally.
        code: Option<i32>,
    },
    /// The current session list (sent on the `/api/ws` events socket).
    Sessions {
        /// All live sessions, oldest first.
        sessions: Vec<SessionMetaWire>,
    },
}

/// Client-to-server messages on a terminal socket.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Raw input bytes (as a UTF-8 string, escape sequences included).
    /// Sending input claims the driver seat.
    Input {
        /// The bytes to write to the PTY.
        data: String,
    },
    /// Resize the PTY; honored only from the driver (or when the seat is
    /// free).
    Resize {
        /// Requested width in columns.
        cols: u16,
        /// Requested height in rows.
        rows: u16,
    },
    /// Ask for a full grid frame (sent after connect/reconnect).
    Refresh,
    /// Close the opened document split for every viewer.
    CloseDoc,
}

/// Format a resolved RGB color as `#rrggbb`.
fn color_hex(color: ix_vt::Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// The style key of a cell: two cells with equal keys join one span.
fn span_of(cell: &ix_vt::Cell) -> Span {
    Span {
        text: String::new(),
        fg: cell.fg.map(color_hex),
        bg: cell.bg.map(color_hex),
        bold: cell.style.bold,
        italic: cell.style.italic,
        underline: cell.style.underline.is_some(),
        inverse: cell.style.inverse,
        dim: cell.style.faint,
        strikethrough: cell.style.strikethrough,
    }
}

/// Whether two spans carry the same style (everything but the text).
fn same_style(a: &Span, b: &Span) -> bool {
    a.fg == b.fg
        && a.bg == b.bg
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.inverse == b.inverse
        && a.dim == b.dim
        && a.strikethrough == b.strikethrough
}

/// Encode one snapshot row as consecutive styled runs.
///
/// Empty cells render as spaces so concatenated span text always covers every
/// column; wide-glyph spacer cells therefore also render as spaces, which
/// keeps column arithmetic exact in fonts that draw CJK at single width and
/// is the v1 tradeoff for fonts that draw them double width.
pub fn encode_row(cells: &[ix_vt::Cell]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for cell in cells {
        let style = span_of(cell);
        if !spans.last().is_some_and(|last| same_style(last, &style)) {
            spans.push(style);
        }
        let span = spans
            .last_mut()
            .expect("span pushed above when the list was empty");
        span.text.push(cell.ch.unwrap_or(' '));
        span.text.extend(cell.combining.iter());
    }
    spans
}

/// A dirty-row delta between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridDiff {
    /// Whether every row is included (first frame or size change).
    pub full: bool,
    /// The changed rows.
    pub changed: Vec<RowUpdate>,
}

/// Diff `next` against `prev`, emitting full content for each changed row.
pub fn diff_snapshots(prev: Option<&ix_vt::Snapshot>, next: &ix_vt::Snapshot) -> GridDiff {
    let full = !prev.is_some_and(|p| p.cols == next.cols && p.rows == next.rows);
    // Rows are indexed in u16 from the start: a viewport is at most
    // `next.rows` (u16) rows tall, so the zip never truncates.
    let changed = (0u16..)
        .zip(next.viewport.iter())
        .filter(|&(y, row)| {
            full || prev.is_none_or(|p| p.viewport.get(usize::from(y)).is_none_or(|old| old != row))
        })
        .map(|(y, row)| RowUpdate {
            y,
            spans: encode_row(row),
        })
        .collect();
    GridDiff { full, changed }
}

/// Map a snapshot cursor to its wire form.
pub fn cursor_wire(cursor: &ix_vt::Cursor) -> Option<CursorWire> {
    cursor.viewport.map(|(x, y)| CursorWire {
        x,
        y,
        visible: cursor.visible,
        shape: cursor.visual_style.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::{ClientMsg, RowUpdate, ServerMsg, Span, diff_snapshots, encode_row};

    fn cell(ch: char) -> ix_vt::Cell {
        ix_vt::Cell {
            ch: Some(ch),
            combining: Vec::new(),
            style: ix_vt::Style::default(),
            fg: None,
            bg: None,
        }
    }

    fn red_cell(ch: char) -> ix_vt::Cell {
        ix_vt::Cell {
            fg: Some(ix_vt::Rgb { r: 255, g: 0, b: 0 }),
            ..cell(ch)
        }
    }

    #[test]
    fn equal_styles_merge_into_one_span() {
        let row = vec![cell('h'), cell('i'), red_cell('!'), red_cell('?')];
        let spans = encode_row(&row);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "hi");
        assert_eq!(spans[1].text, "!?");
        assert_eq!(spans[1].fg.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn empty_cells_render_as_spaces_covering_all_columns() {
        let row = vec![
            cell('x'),
            ix_vt::Cell {
                ch: None,
                combining: Vec::new(),
                style: ix_vt::Style::default(),
                fg: None,
                bg: None,
            },
        ];
        let spans = encode_row(&row);
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "x ");
    }

    fn snapshot(rows: &[&str]) -> ix_vt::Snapshot {
        ix_vt::Snapshot {
            cols: u16::try_from(rows.first().map_or(0, |r| r.len())).expect("test rows fit u16"),
            rows: u16::try_from(rows.len()).expect("test rows fit u16"),
            viewport: rows.iter().map(|r| r.chars().map(cell).collect()).collect(),
            scrollback: 0,
            cursor: ix_vt::Cursor {
                visible: true,
                blinking: true,
                visual_style: ix_vt::CursorVisualStyle::Block,
                viewport: Some((0, 0)),
            },
        }
    }

    #[test]
    fn first_frame_is_full() {
        let next = snapshot(&["ab", "cd"]);
        let diff = diff_snapshots(None, &next);
        assert!(diff.full);
        assert_eq!(diff.changed.len(), 2);
    }

    #[test]
    fn only_changed_rows_ship_in_a_patch_frame() {
        let prev = snapshot(&["ab", "cd", "ef"]);
        let next = snapshot(&["ab", "cX", "ef"]);
        let diff = diff_snapshots(Some(&prev), &next);
        assert!(!diff.full);
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].y, 1);
        assert_eq!(diff.changed[0].spans[0].text, "cX");
    }

    #[test]
    fn size_change_forces_a_full_frame() {
        let prev = snapshot(&["ab", "cd"]);
        let next = snapshot(&["abc", "def"]);
        let diff = diff_snapshots(Some(&prev), &next);
        assert!(diff.full);
        assert_eq!(diff.changed.len(), 2);
    }

    // The wire format is a contract with the Svelte client (and any future
    // one); these pin the exact bytes.

    #[test]
    fn grid_frame_serializes_byte_exact() {
        let msg = ServerMsg::Grid {
            seq: 7,
            cols: 2,
            rows: 1,
            full: false,
            changed: vec![RowUpdate {
                y: 0,
                spans: vec![Span {
                    text: "hi".to_owned(),
                    fg: Some("#ff0000".to_owned()),
                    bold: true,
                    ..Span::default()
                }],
            }],
            cursor: Some(super::CursorWire {
                x: 1,
                y: 0,
                visible: true,
                shape: super::CursorShapeWire::Block,
            }),
            app_cursor: false,
        };
        assert_eq!(
            serde_json::to_string(&msg).expect("serializes"),
            r##"{"type":"grid","seq":7,"cols":2,"rows":1,"full":false,"changed":[{"y":0,"spans":[{"text":"hi","fg":"#ff0000","bold":true}]}],"cursor":{"x":1,"y":0,"visible":true,"shape":"block"},"app_cursor":false}"##
        );
    }

    #[test]
    fn driver_and_open_serialize_byte_exact() {
        let driver = ServerMsg::Driver {
            conn: Some("3".to_owned()),
            cols: 120,
            rows: 32,
        };
        assert_eq!(
            serde_json::to_string(&driver).expect("serializes"),
            r#"{"type":"driver","conn":"3","cols":120,"rows":32}"#
        );
        let open = ServerMsg::Open {
            path: Some("/tmp/x.html".to_owned()),
            nonce: 4,
        };
        assert_eq!(
            serde_json::to_string(&open).expect("serializes"),
            r#"{"type":"open","path":"/tmp/x.html","nonce":4}"#
        );
    }

    #[test]
    fn client_messages_parse() {
        assert_eq!(
            serde_json::from_str::<ClientMsg>(r#"{"type":"input","data":"ls\r"}"#).expect("parses"),
            ClientMsg::Input {
                data: "ls\r".to_owned()
            }
        );
        assert_eq!(
            serde_json::from_str::<ClientMsg>(r#"{"type":"resize","cols":80,"rows":24}"#)
                .expect("parses"),
            ClientMsg::Resize { cols: 80, rows: 24 }
        );
        assert_eq!(
            serde_json::from_str::<ClientMsg>(r#"{"type":"refresh"}"#).expect("parses"),
            ClientMsg::Refresh
        );
        assert_eq!(
            serde_json::from_str::<ClientMsg>(r#"{"type":"close_doc"}"#).expect("parses"),
            ClientMsg::CloseDoc
        );
    }
}
