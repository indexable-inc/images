//! Safe Rust wrapper over [libghostty-vt], ghostty's terminal VT engine.
//!
//! This crate owns the safe surface over the raw FFI in [`ix_vt_sys`]. It
//! mirrors the community [`uzaaft/libghostty-rs`] `render.rs` shape: create a
//! [`Terminal`], feed it VT bytes with [`Terminal::vt_write`], [`resize`] it,
//! and capture a [`Snapshot`] of the render state with [`Terminal::render`].
//!
//! The snapshot exposes the viewport as styled [`Cell`]s, the scrollback size,
//! and the [`Cursor`] (viewport position, visibility, blink, and visual style).
//! Everything is owned and copied out of the C structures, so a snapshot stays
//! valid after the terminal is written to or dropped.
//!
//! [libghostty-vt]: https://ghostty.org/
//! [`uzaaft/libghostty-rs`]: https://github.com/uzaaft/libghostty-rs
//! [`resize`]: Terminal::resize

use std::cell::UnsafeCell;
use std::ffi::{CStr, c_void};
use std::fmt;
use std::os::raw::c_char;
use std::ptr;

use ix_vt_sys as sys;

/// An error returned by a libghostty-vt call.
///
/// Wraps the non-success values of the C `GhosttyResult` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The library failed to allocate memory (`GHOSTTY_OUT_OF_MEMORY`).
    OutOfMemory,
    /// An argument was invalid for the call (`GHOSTTY_INVALID_VALUE`).
    InvalidValue,
    /// A fixed-size output buffer was too small (`GHOSTTY_OUT_OF_SPACE`).
    OutOfSpace,
    /// The call succeeded but there was no value to return
    /// (`GHOSTTY_NO_VALUE`).
    NoValue,
    /// A result code outside the documented enum.
    Unknown(i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => f.write_str("libghostty-vt: out of memory"),
            Self::InvalidValue => f.write_str("libghostty-vt: invalid value"),
            Self::OutOfSpace => f.write_str("libghostty-vt: out of space"),
            Self::NoValue => f.write_str("libghostty-vt: no value"),
            Self::Unknown(code) => write!(f, "libghostty-vt: unknown result code {code}"),
        }
    }
}

impl std::error::Error for Error {}

/// The result of an `ix-vt` operation.
pub type Result<T> = std::result::Result<T, Error>;

/// Convert a raw `GhosttyResult` into a `Result<()>`.
const fn check(result: sys::GhosttyResult) -> Result<()> {
    match result {
        sys::GhosttyResult::GHOSTTY_SUCCESS => Ok(()),
        sys::GhosttyResult::GHOSTTY_OUT_OF_MEMORY => Err(Error::OutOfMemory),
        sys::GhosttyResult::GHOSTTY_INVALID_VALUE => Err(Error::InvalidValue),
        sys::GhosttyResult::GHOSTTY_OUT_OF_SPACE => Err(Error::OutOfSpace),
        sys::GhosttyResult::GHOSTTY_NO_VALUE => Err(Error::NoValue),
        // The `_MAX_VALUE` sentinel only pins the enum's ABI width; the
        // library never returns it.
        sys::GhosttyResult::GHOSTTY_RESULT_MAX_VALUE => Err(Error::Unknown(i32::MAX)),
    }
}

/// An RGB color with 8 bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red channel (0-255).
    pub r: u8,
    /// Green channel (0-255).
    pub g: u8,
    /// Blue channel (0-255).
    pub b: u8,
}

impl From<sys::GhosttyColorRgb> for Rgb {
    fn from(c: sys::GhosttyColorRgb) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

/// A style color: unset, a 256-color palette index, or a direct RGB value.
///
/// This preserves the cell's declared color before palette resolution. Use
/// [`Cell::fg`] / [`Cell::bg`] for the resolved RGB the renderer should draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StyleColor {
    /// No color set; the renderer should use its default.
    #[default]
    None,
    /// A 256-color palette index (`GHOSTTY_STYLE_COLOR_PALETTE`).
    Palette(u8),
    /// A direct RGB color (`GHOSTTY_STYLE_COLOR_RGB`).
    Rgb(Rgb),
}

impl StyleColor {
    /// Read a tagged style color out of its C union.
    ///
    /// # Safety
    /// `color.value` must be initialized for the variant named by `color.tag`,
    /// which holds for any value libghostty-vt writes into a `GhosttyStyle`.
    unsafe fn from_raw(color: sys::GhosttyStyleColor) -> Self {
        match color.tag {
            // The MAX_VALUE arm is an ABI-width sentinel; never a real tag.
            sys::GhosttyStyleColorTag::GHOSTTY_STYLE_COLOR_NONE
            | sys::GhosttyStyleColorTag::GHOSTTY_STYLE_COLOR_TAG_MAX_VALUE => Self::None,
            sys::GhosttyStyleColorTag::GHOSTTY_STYLE_COLOR_PALETTE => {
                Self::Palette(unsafe { color.value.palette })
            }
            sys::GhosttyStyleColorTag::GHOSTTY_STYLE_COLOR_RGB => {
                Self::Rgb(unsafe { color.value.rgb }.into())
            }
        }
    }
}

/// The text-decoration flags and declared colors of a cell.
///
/// Booleans mirror the SGR attributes ghostty tracks; [`Style::underline`] is
/// non-`None` when any underline style is set.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one bool per independent SGR attribute ghostty exposes; they are not a state enum"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    /// Bold (SGR 1).
    pub bold: bool,
    /// Italic (SGR 3).
    pub italic: bool,
    /// Faint / dim (SGR 2).
    pub faint: bool,
    /// Blinking (SGR 5).
    pub blink: bool,
    /// Inverse / reverse video (SGR 7).
    pub inverse: bool,
    /// Invisible / concealed (SGR 8).
    pub invisible: bool,
    /// Strikethrough (SGR 9).
    pub strikethrough: bool,
    /// Overline (SGR 53).
    pub overline: bool,
    /// Underline style, if any (the raw `GhosttySgrUnderline` value; non-zero
    /// means some underline is set).
    pub underline: Option<u8>,
    /// The cell's declared foreground color before palette resolution.
    pub fg_color: StyleColor,
    /// The cell's declared background color before palette resolution.
    pub bg_color: StyleColor,
    /// The cell's declared underline color before palette resolution.
    pub underline_color: StyleColor,
}

impl Style {
    /// Build an owned [`Style`] from a C `GhosttyStyle`.
    ///
    /// # Safety
    /// `raw` must be a fully initialized `GhosttyStyle` as written by
    /// libghostty-vt (its color unions tagged consistently).
    unsafe fn from_raw(raw: &sys::GhosttyStyle) -> Self {
        Self {
            bold: raw.bold,
            italic: raw.italic,
            faint: raw.faint,
            blink: raw.blink,
            inverse: raw.inverse,
            invisible: raw.invisible,
            strikethrough: raw.strikethrough,
            overline: raw.overline,
            // `raw.underline` is libghostty-vt's `GhosttySgrUnderline` enum
            // (values 0..=5). `try_from(..).ok()` keeps the value when it fits
            // and yields `None` (not a silent default) for any out-of-range
            // value; `filter` drops 0, which means "no underline".
            underline: u8::try_from(raw.underline).ok().filter(|&u| u != 0),
            fg_color: unsafe { StyleColor::from_raw(raw.fg_color) },
            bg_color: unsafe { StyleColor::from_raw(raw.bg_color) },
            underline_color: unsafe { StyleColor::from_raw(raw.underline_color) },
        }
    }
}

/// How many columns the character in a cell occupies, and which of those
/// columns this cell is.
///
/// Ghostty answers this once, when the character is laid onto the grid, and
/// the answer is a Unicode width table plus grapheme rules plus the mode
/// bits the application set. Anything downstream that needs to know where a
/// character starts and ends -- re-wrapping captured output at a new width is
/// the case this was surfaced for -- reads that answer instead of computing a
/// second one, because two width tables that disagree draw two different
/// pictures of the same bytes and only one of them is what the program saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CellWide {
    /// One column wide, which is nearly every cell.
    #[default]
    Narrow,
    /// The first column of a two-column character.
    Wide,
    /// The second column of a two-column character. Carries no glyph of its
    /// own and must never be separated from the [`CellWide::Wide`] before it.
    SpacerTail,
    /// The last column of a soft-wrapped row, left blank because the next
    /// character was two columns wide and one column was left. It is padding
    /// the terminal inserted, not something the program printed, so a reflow
    /// that rejoins the row has to drop it.
    SpacerHead,
}

impl CellWide {
    /// Map libghostty-vt's `GhosttyCellWide` tag.
    ///
    /// Taken as a raw `u32` and matched rather than materialized as the C
    /// enum, for the reason [`Terminal::row_semantic_prompt`] gives: forming
    /// a Rust enum from an out-of-range FFI value is UB.
    const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Wide,
            2 => Self::SpacerTail,
            3 => Self::SpacerHead,
            _ => Self::Narrow,
        }
    }
}

/// A single rendered terminal cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The base codepoint of the cell, or `None` for an empty cell.
    pub ch: Option<char>,
    /// Any extra grapheme codepoints combined onto the base codepoint.
    pub combining: Vec<char>,
    /// The cell's style flags and declared colors.
    pub style: Style,
    /// The resolved foreground RGB, with palette indices already looked up.
    /// `None` means the cell uses the terminal default foreground.
    pub fg: Option<Rgb>,
    /// The resolved background RGB, with palette indices already looked up.
    /// `None` means the cell uses the terminal default background.
    pub bg: Option<Rgb>,
    /// The cell's OSC 8 hyperlink URI, or `None` when the cell is not part
    /// of a hyperlink. Per-cell and exact: unlike the row-level
    /// `GHOSTTY_ROW_DATA_HYPERLINK` flag (which may false-positive), this is
    /// only `Some` when the library resolved a real URI for the cell.
    pub hyperlink: Option<String>,
    /// Which column of its character this cell is; see [`CellWide`].
    pub wide: CellWide,
}

/// The terminal cursor's visual style (the shape requested via DECSCUSR).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorVisualStyle {
    /// A vertical bar (DECSCUSR 5/6).
    Bar,
    /// A filled block (DECSCUSR 0/1/2).
    Block,
    /// An underline (DECSCUSR 3/4).
    Underline,
    /// A hollow block, drawn when the terminal is unfocused.
    BlockHollow,
}

impl From<sys::GhosttyRenderStateCursorVisualStyle> for CursorVisualStyle {
    fn from(s: sys::GhosttyRenderStateCursorVisualStyle) -> Self {
        use sys::GhosttyRenderStateCursorVisualStyle as Raw;
        match s {
            Raw::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => Self::Bar,
            // The MAX_VALUE arm is an ABI-width sentinel; never a real style.
            Raw::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK
            | Raw::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_MAX_VALUE => Self::Block,
            Raw::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => Self::Underline,
            Raw::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => Self::BlockHollow,
        }
    }
}

/// The cursor state captured in a [`Snapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Whether the cursor is currently visible.
    pub visible: bool,
    /// Whether the cursor is blinking.
    pub blinking: bool,
    /// The cursor's visual style.
    pub visual_style: CursorVisualStyle,
    /// The cursor's `(col, row)` position within the viewport, or `None` when
    /// the cursor is scrolled out of the visible area.
    pub viewport: Option<(u16, u16)>,
}

/// One row of a [`Snapshot`]: its cells, and whether the line it belongs to
/// carries on into the next row.
///
/// The wrap flag is the difference between a grid and a transcript. A grid
/// row is just a row, but a reader of captured output needs to know why a
/// row ended: because the terminal ran out of columns, in which case the
/// text is one line that can be laid out again at another width, or because
/// the program printed a newline, in which case joining it to the next row
/// would scramble anything that positioned its output deliberately. Nothing
/// else in the row distinguishes the two.
///
/// Derefs to its cells, so every reader that only wants the row's content
/// keeps reading it as a slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The row's cells, `cols` of them.
    pub cells: Vec<Cell>,
    /// The row was soft-wrapped: its line continues on the row below.
    pub wrapped: bool,
}

impl std::ops::Deref for Row {
    type Target = [Cell];

    fn deref(&self) -> &Self::Target {
        &self.cells
    }
}

impl<'a> IntoIterator for &'a Row {
    type Item = &'a Cell;
    type IntoIter = std::slice::Iter<'a, Cell>;

    fn into_iter(self) -> Self::IntoIter {
        self.cells.iter()
    }
}

/// An immutable snapshot of a terminal's render state.
///
/// Produced by [`Terminal::render`]. All data is copied out of the C render
/// state, so the snapshot is independent of later terminal writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Viewport width in columns.
    pub cols: u16,
    /// Viewport height in rows.
    pub rows: u16,
    /// The visible viewport as `rows` rows, each `cols` cells wide.
    pub viewport: Vec<Row>,
    /// Number of rows held in scrollback above the viewport.
    pub scrollback: u64,
    /// The cursor state.
    pub cursor: Cursor,
}

/// How to move the viewport over the scrollback, for
/// [`Terminal::scroll_viewport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollViewport {
    /// Scroll to the oldest scrollback row.
    Top,
    /// Scroll back to the active (bottom) viewport.
    Bottom,
    /// Scroll by `delta` rows: positive scrolls down toward the bottom,
    /// negative scrolls up into history.
    Delta(isize),
}

/// The mouse-reporting flavor an application has requested via DEC private
/// modes 9/1000/1002/1003 (see [`Terminal::mouse_reporting`]).
///
/// The variants are ordered by how much the application asked to see; when
/// several modes are set at once the strongest wins, matching how terminals
/// dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MouseReporting {
    /// No mouse mode set: the terminal owns the mouse (scrollback, selection).
    None,
    /// X10 compatibility mode (DECSET 9): presses only, no release events.
    X10,
    /// Normal tracking (DECSET 1000): presses and releases.
    Normal,
    /// Button-event tracking (DECSET 1002): 1000 plus drag motion while a
    /// button is held.
    Button,
    /// Any-event tracking (DECSET 1003): 1002 plus all motion.
    Any,
}

/// Scrollbar geometry for the terminal viewport, in rows: `total` scrollable
/// rows, the viewport's `offset` from the top, and its `len`. The viewport is
/// at the live bottom iff `offset + len == total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scrollbar {
    /// Total rows in the scrollable area (scrollback plus active screen).
    pub total: u64,
    /// Rows above the top of the viewport.
    pub offset: u64,
    /// Viewport height in rows.
    pub len: u64,
}

/// Kitty keyboard protocol progressive-enhancement flags.
///
/// Set by the application via `CSI = flags ; mode u` / `CSI > flags u` (see
/// [`Terminal::kitty_keyboard_flags`]). A zero value means the protocol is
/// disabled and legacy encoding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KittyKeyboardFlags(u8);

impl KittyKeyboardFlags {
    /// Disambiguate escape codes (`GHOSTTY_KITTY_KEY_DISAMBIGUATE`).
    pub const DISAMBIGUATE: Self = Self(1);
    /// Report key repeat and release events (`GHOSTTY_KITTY_KEY_REPORT_EVENTS`).
    pub const REPORT_EVENTS: Self = Self(2);
    /// Report shifted and base-layout alternate keys
    /// (`GHOSTTY_KITTY_KEY_REPORT_ALTERNATES`).
    pub const REPORT_ALTERNATES: Self = Self(4);
    /// Report all keys as escape codes (`GHOSTTY_KITTY_KEY_REPORT_ALL`).
    pub const REPORT_ALL: Self = Self(8);
    /// Report associated text with key events
    /// (`GHOSTTY_KITTY_KEY_REPORT_ASSOCIATED`).
    pub const REPORT_ASSOCIATED: Self = Self(16);

    /// The raw bitmask (`GhosttyKittyKeyFlags`), e.g. for shipping on a wire.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Rebuild from a raw bitmask (the inverse of [`bits`](Self::bits)).
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Whether every flag in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag is set (legacy keyboard encoding).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// The OSC 133 semantic-prompt classification of a row (shell integration
/// marks; see [`Terminal::row_semantic_prompt`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RowSemanticPrompt {
    /// No prompt cells in the row (regular output or input).
    #[default]
    None,
    /// A primary prompt line (the row an `OSC 133;A` mark opened).
    Prompt,
    /// A continuation line of a multi-row prompt.
    PromptContinuation,
}

/// Addresses one row of the terminal grid for state queries, in one of
/// ghostty's coordinate spaces (`GhosttyPointTag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowLocation {
    /// A row of the active area (where the cursor can move), 0-indexed from
    /// its top.
    Active(u16),
    /// A row of the visible viewport (moves when scrolled), 0-indexed from
    /// its top.
    Viewport(u16),
    /// A row of the full screen including scrollback, 0-indexed from the
    /// oldest history row.
    Screen(u32),
    /// A row of the scrollback history only (before the active area).
    History(u32),
}

impl RowLocation {
    /// The raw tagged point at column 0 of the addressed row.
    fn point(self) -> sys::GhosttyPoint {
        let (tag, y) = match self {
            Self::Active(y) => (sys::GhosttyPointTag::GHOSTTY_POINT_TAG_ACTIVE, u32::from(y)),
            Self::Viewport(y) => (
                sys::GhosttyPointTag::GHOSTTY_POINT_TAG_VIEWPORT,
                u32::from(y),
            ),
            Self::Screen(y) => (sys::GhosttyPointTag::GHOSTTY_POINT_TAG_SCREEN, y),
            Self::History(y) => (sys::GhosttyPointTag::GHOSTTY_POINT_TAG_HISTORY, y),
        };
        sys::GhosttyPoint {
            tag,
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate { x: 0, y },
            },
        }
    }
}

/// Options for creating a [`Terminal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    /// Number of columns (width in cells).
    pub cols: u16,
    /// Number of rows (height in cells).
    pub rows: u16,
    /// Memory budget for the grid — scrollback plus active screen — in
    /// **bytes**.
    ///
    /// Bytes, not rows, whatever `ghostty/vt/terminal.h` says about
    /// `max_scrollback` ("Maximum number of lines to keep in scrollback
    /// history"; the header comment is wrong, ix#9031). The C field goes
    /// straight to ghostty's `Screen.init`, whose own doc comment reads
    /// "max_scrollback is the amount of scrollback to keep in bytes", and
    /// from there to `PageList.init`'s `max_size`, "the maximum number of
    /// bytes that will be allocated for pages". ghostty's user-facing config
    /// agrees: `scrollback-limit` is "the size of the scrollback buffer in
    /// bytes", default 10MB.
    ///
    /// Two consequences a caller has to know:
    ///
    /// - A budget too small to hold the active area is silently raised to fit
    ///   it — `PageList.maxSize` returns `@max(explicit_max_size,
    ///   min_max_size)` — so a row count passed here is not rejected, it is
    ///   ignored. That is the whole of ix#9031: `10_000` and `100_000` both
    ///   land under the floor and buy the same ~1000 rows.
    /// - Zero means *no scrollback at all*, not unlimited.
    ///
    /// Size this from a row count with [`scrollback_bytes_for_lines`].
    pub max_scrollback_bytes: usize,
}

/// Bytes of grid data one row occupies at `cols` columns.
///
/// ghostty's `Row` and `Cell` are both `packed struct(u64)`
/// (`terminal/page.zig`), and `Capacity.adjust` fits rows into a page at
/// exactly `@bitSizeOf(Row) + @bitSizeOf(Cell) * cols` bits each.
const fn row_grid_bytes(cols: u16) -> usize {
    const ROW_BYTES: usize = 8;
    const CELL_BYTES: usize = 8;
    ROW_BYTES + CELL_BYTES * cols as usize
}

/// What a retained row actually costs against the budget, as a percentage of
/// its grid data.
///
/// A page's style, grapheme, string and hyperlink arenas are laid out from its
/// end and the grid only gets what is left
/// (`Capacity.availableBitsForGrid`) — the string arena takes the largest
/// share — so a budget buys fewer rows than [`row_grid_bytes`] predicts. The
/// factor does not depend on the column count: `Capacity.adjust` changes only
/// a page's row count, never its arenas.
///
/// Measured against libghostty-vt 1.3.2 between 168% and 184% from 80 to 400
/// columns; 250% is that with room for ghostty to grow an arena.
/// `scrollback_bytes_for_lines_delivers_the_rows_it_promises` is what fails if
/// it ever grows past this — checked by setting this to 100, which keeps 5,451
/// rows where 10,000 were asked for.
const ROW_BUDGET_COST_PERCENT: usize = 250;

/// The [`TerminalOptions::max_scrollback_bytes`] budget that holds at least
/// `lines` rows of grid on a terminal `cols` columns wide.
///
/// ghostty budgets the grid in bytes, not rows (see
/// [`TerminalOptions::max_scrollback_bytes`]), so a row count only becomes a
/// byte count once the width is known: 10,000 rows of 80 columns and 10,000
/// rows of 400 columns differ by 5x. `lines` counts the whole scrollable area
/// the way ghostty's `scrollback-limit` does ("this also includes the active
/// screen"), so it is what [`Scrollbar::total`] reports rather than the rows
/// above the viewport.
///
/// The result is a floor, not a target: ghostty prunes a page at a time and
/// the budget is deliberately generous, so the terminal keeps somewhere
/// between `lines` and about 1.4x `lines`. A caller that wants a hard memory
/// cap wants [`TerminalOptions::max_scrollback_bytes`] directly.
///
/// The width is the one passed here, and libghostty-vt has no option for
/// changing the budget afterwards (`GhosttyTerminalOption` has no entry for
/// it), so a terminal that is later [`resize`]d wider than `cols` holds
/// proportionally fewer rows than it was sized for.
///
/// [`resize`]: Terminal::resize
#[must_use]
pub const fn scrollback_bytes_for_lines(lines: usize, cols: u16) -> usize {
    row_grid_bytes(cols)
        .saturating_mul(lines)
        .saturating_mul(ROW_BUDGET_COST_PERCENT)
        / 100
}

/// A terminal VT engine instance.
///
/// Owns the underlying `GhosttyTerminal` and frees it on drop.
pub struct Terminal {
    raw: sys::GhosttyTerminal,
    /// Bytes the terminal wants written back to the PTY: query replies (DSR
    /// cursor position, DA1, DECRQM, XTVERSION, the kitty-keyboard flags
    /// query) and in-band size reports. Filled by the `write_pty` callback,
    /// which ghostty invokes synchronously inside [`vt_write`]/[`resize`] on
    /// the owning thread, and emptied by [`drain_responses`].
    ///
    /// Boxed so the address handed to C as userdata survives moves of
    /// `Terminal`; `UnsafeCell` because the C side writes through that
    /// pointer while no Rust reference to the buffer exists.
    ///
    /// [`vt_write`]: Terminal::vt_write
    /// [`resize`]: Terminal::resize
    /// [`drain_responses`]: Terminal::drain_responses
    responses: Box<UnsafeCell<Vec<u8>>>,
}

/// The `write_pty` callback registered on every [`Terminal`]: appends the
/// response bytes to the buffer behind `userdata`.
///
/// # Safety
/// `userdata` must be the `UnsafeCell<Vec<u8>>` registered at construction,
/// and the call must not race a Rust borrow of that buffer. Both hold:
/// ghostty only invokes the callback synchronously inside
/// `ghostty_terminal_vt_write`/`ghostty_terminal_resize` (the C header
/// forbids reentrancy), `Terminal` is `!Send`/`!Sync`, and no Rust reference
/// to the buffer is live across those calls.
unsafe extern "C" fn collect_response(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if data.is_null() || len == 0 {
        return;
    }
    let buffer = userdata.cast::<UnsafeCell<Vec<u8>>>();
    // SAFETY: see the function docs; `buffer` points at the live
    // `UnsafeCell<Vec<u8>>` owned by the `Terminal` being written to.
    unsafe { (*(*buffer).get()).extend_from_slice(std::slice::from_raw_parts(data, len)) };
}

// `Terminal` is intentionally left `!Send` and `!Sync` (the raw pointer makes it
// so by default). libghostty-vt's terminal has thread affinity, so the handle
// must stay on the thread that created it; a caller that needs it from async or
// another thread owns it on a pinned thread behind a channel API rather than
// moving the handle. Do not add an `unsafe impl Send`/`Sync`.

/// DECCKM (DEC private mode 1, "cursor keys") in libghostty's packed mode
/// encoding. The C header defines it as `ghostty_mode_new(1, ansi=false)`, i.e.
/// `(value & 0x7FFF) | ((ansi as u16) << 15)`, which for value 1 / DEC-private
/// is simply `1`. See `ghostty/vt/modes.h` (`GHOSTTY_MODE_DECCKM`).
const DECCKM: sys::GhosttyMode = 1;

/// The DEC private mouse modes, in libghostty's packed mode encoding (the
/// DEC-private packing is the value itself; see [`DECCKM`]). Names follow
/// ghostty's `modes.zig` table.
const MOUSE_EVENT_X10: sys::GhosttyMode = 9;
const MOUSE_EVENT_NORMAL: sys::GhosttyMode = 1000;
const MOUSE_EVENT_BUTTON: sys::GhosttyMode = 1002;
const MOUSE_EVENT_ANY: sys::GhosttyMode = 1003;
/// DECSET 1006: report mouse events in the SGR encoding (`CSI < b;x;y M/m`),
/// which is unambiguous and unbounded, unlike the legacy X10 byte encoding.
const MOUSE_FORMAT_SGR: sys::GhosttyMode = 1006;
/// DECSET 2004: bracketed paste. When set, the application expects pasted
/// text wrapped in `ESC [ 200 ~` / `ESC [ 201 ~` guards.
const BRACKETED_PASTE: sys::GhosttyMode = 2004;
/// DECSET 1004: focus event reporting. When set, the application expects
/// `ESC [ I` / `ESC [ O` on terminal focus gain/loss.
const FOCUS_EVENT: sys::GhosttyMode = 1004;
/// DEC private mode 2026: synchronized output. While set, a renderer should
/// hold frames so the application can update the screen atomically.
const SYNCHRONIZED_OUTPUT: sys::GhosttyMode = 2026;

impl Terminal {
    /// Create a terminal sized `rows` by `cols` whose grid — scrollback plus
    /// active screen — may occupy `max_scrollback_bytes`.
    ///
    /// Bytes, not rows; [`scrollback_bytes_for_lines`] converts, and
    /// [`TerminalOptions::max_scrollback_bytes`] explains why the distinction
    /// is not cosmetic.
    ///
    /// The argument order is `(rows, cols, ...)` to read like a screen size;
    /// the underlying C struct stores `cols`/`rows` separately, so there is no
    /// ambiguity once constructed.
    ///
    /// # Errors
    /// Returns an [`Error`] if ghostty cannot allocate the terminal (see
    /// [`Self::with_options`]).
    pub fn new(rows: u16, cols: u16, max_scrollback_bytes: usize) -> Result<Self> {
        Self::with_options(TerminalOptions {
            cols,
            rows,
            max_scrollback_bytes,
        })
    }

    /// Create a terminal from explicit [`TerminalOptions`].
    ///
    /// # Errors
    /// Returns [`Error::OutOfMemory`] if ghostty cannot allocate the terminal,
    /// or [`Error::InvalidValue`] if it rejects the options.
    pub fn with_options(options: TerminalOptions) -> Result<Self> {
        let mut raw: sys::GhosttyTerminal = ptr::null_mut();
        let opts = sys::GhosttyTerminalOptions {
            cols: options.cols,
            rows: options.rows,
            max_scrollback: options.max_scrollback_bytes,
        };
        // Passing a null allocator selects the default (libc malloc/free).
        check(unsafe { sys::ghostty_terminal_new(ptr::null(), &raw mut raw, opts) })?;
        let terminal = Self {
            raw,
            responses: Box::new(UnsafeCell::new(Vec::new())),
        };

        // Answer terminal queries (ix#8117): without a `write_pty` callback
        // ghostty silently drops every sequence that needs a reply, and
        // anything that queries the terminal at startup (reedline reading
        // the cursor position, for one) hangs forever. Register the
        // collector unconditionally; `drain_responses` is the read side.
        check(unsafe {
            sys::ghostty_terminal_set(
                terminal.raw,
                sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_USERDATA,
                terminal
                    .responses
                    .as_ref()
                    .get()
                    .cast::<c_void>()
                    .cast_const(),
            )
        })?;
        let write_pty: unsafe extern "C" fn(sys::GhosttyTerminal, *mut c_void, *const u8, usize) =
            collect_response;
        // Function-pointer options take the pointer itself as the value.
        check(unsafe {
            sys::ghostty_terminal_set(
                terminal.raw,
                sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                write_pty as *const c_void,
            )
        })?;
        Ok(terminal)
    }

    /// Take the bytes the terminal wants written back to the PTY.
    ///
    /// Feeding VT input with [`vt_write`](Self::vt_write) (or resizing with
    /// mode 2048 enabled) can make the terminal emit replies: DSR cursor
    /// position, DA1/DA2 device attributes, DECRQM mode reports, XTVERSION,
    /// the kitty-keyboard flags reply, and in-band size reports. They
    /// accumulate here in emission order; draining returns everything
    /// buffered and empties the buffer. The caller owns writing them to the
    /// application's input (the PTY master in a server).
    ///
    /// Returns an empty vector when nothing queried the terminal.
    pub fn drain_responses(&mut self) -> Vec<u8> {
        std::mem::take(self.responses.get_mut())
    }

    /// Feed raw VT bytes (escape sequences and text) into the terminal.
    pub fn vt_write(&mut self, data: &[u8]) {
        unsafe { sys::ghostty_terminal_vt_write(self.raw, data.as_ptr(), data.len()) };
    }

    /// Resize the terminal to `rows` by `cols`. Both must be greater than zero.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if `rows` or `cols` is zero, or another
    /// [`Error`] if ghostty rejects the resize.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        // Cell pixel dimensions are unknown to a headless embedder; zero is
        // the protocol convention for "unknown" in the size reports ghostty
        // derives from them (mode 2048, XTWINOPS).
        check(unsafe { sys::ghostty_terminal_resize(self.raw, cols, rows, 0, 0) })
    }

    /// Move the viewport over the scrollback history.
    ///
    /// [`render`](Self::render) always reads the current viewport, so a caller
    /// that wants scrollback content scrolls the viewport up, renders, and
    /// scrolls back. The viewport position is terminal state, so this takes
    /// `&mut self`.
    pub fn scroll_viewport(&mut self, behavior: ScrollViewport) {
        let raw = match behavior {
            ScrollViewport::Top => sys::GhosttyTerminalScrollViewport {
                tag: sys::GhosttyTerminalScrollViewportTag::GHOSTTY_SCROLL_VIEWPORT_TOP,
                value: sys::GhosttyTerminalScrollViewportValue { _padding: [0, 0] },
            },
            ScrollViewport::Bottom => sys::GhosttyTerminalScrollViewport {
                tag: sys::GhosttyTerminalScrollViewportTag::GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
                value: sys::GhosttyTerminalScrollViewportValue { _padding: [0, 0] },
            },
            ScrollViewport::Delta(delta) => sys::GhosttyTerminalScrollViewport {
                tag: sys::GhosttyTerminalScrollViewportTag::GHOSTTY_SCROLL_VIEWPORT_DELTA,
                value: sys::GhosttyTerminalScrollViewportValue { delta },
            },
        };
        unsafe { sys::ghostty_terminal_scroll_viewport(self.raw, raw) };
    }

    /// Read a scalar value of type `T` from the terminal via
    /// `ghostty_terminal_get`.
    ///
    /// # Safety
    /// `T` must match the C output type documented for `data`.
    unsafe fn get<T>(&self, data: sys::GhosttyTerminalData) -> Result<T> {
        let mut out = std::mem::MaybeUninit::<T>::uninit();
        check(unsafe {
            sys::ghostty_terminal_get(self.raw, data, out.as_mut_ptr().cast::<c_void>())
        })?;
        Ok(unsafe { out.assume_init() })
    }

    /// Capture an owned [`Snapshot`] of the current render state.
    ///
    /// # Errors
    /// Returns an [`Error`] if the render state cannot be allocated, updated
    /// from the terminal, or read back.
    pub fn render(&self) -> Result<Snapshot> {
        let state = RenderState::new()?;
        check(unsafe { sys::ghostty_render_state_update(state.raw, self.raw) })?;

        let cols: u16 =
            unsafe { state.get(sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_COLS) }?;
        let rows: u16 =
            unsafe { state.get(sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_ROWS) }?;

        let cursor = state.cursor()?;
        let viewport = state.viewport(cols)?;
        let scrollback = self.scrollback()?;

        Ok(Snapshot {
            cols,
            rows,
            viewport,
            scrollback,
            cursor,
        })
    }

    /// Number of scrollback rows above the viewport.
    ///
    /// Derived from the terminal scrollbar: `total - len`, where `total` is the
    /// scrollable area and `len` is the visible viewport height.
    fn scrollback(&self) -> Result<u64> {
        let bar: sys::GhosttyTerminalScrollbar =
            unsafe { self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_SCROLLBAR) }?;
        Ok(bar.total.saturating_sub(bar.len))
    }

    /// Whether the program has enabled DECCKM (DEC private mode 1, "cursor
    /// keys"). When set, the program (via terminfo `smkx`, as ncurses, vim, and
    /// less do on entry) expects the cursor keys in application form
    /// (`ESC O A`..`ESC O D`) instead of normal form (`ESC [ A`..`ESC [ D`), so
    /// an input driver must emit the matching form or the program never sees the
    /// arrow keys at all.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if ghostty rejects the mode query.
    pub fn application_cursor_keys(&self) -> Result<bool> {
        self.mode_enabled(DECCKM)
    }

    /// The mouse-reporting flavor the application has requested (DEC private
    /// modes 9/1000/1002/1003). [`MouseReporting::None`] means the terminal
    /// keeps the mouse: wheel input should scroll scrollback rather than be
    /// forwarded. When set, an input driver forwards encoded mouse events —
    /// in the SGR encoding iff [`sgr_mouse`](Self::sgr_mouse) is also set.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if ghostty rejects a mode query.
    pub fn mouse_reporting(&self) -> Result<MouseReporting> {
        // Strongest first: TUIs commonly set several (vim sets 1000+1002),
        // and the strongest one determines which events they expect.
        for (mode, reporting) in [
            (MOUSE_EVENT_ANY, MouseReporting::Any),
            (MOUSE_EVENT_BUTTON, MouseReporting::Button),
            (MOUSE_EVENT_NORMAL, MouseReporting::Normal),
            (MOUSE_EVENT_X10, MouseReporting::X10),
        ] {
            if self.mode_enabled(mode)? {
                return Ok(reporting);
            }
        }
        Ok(MouseReporting::None)
    }

    /// Whether DECSET 1006 (SGR mouse encoding) is set. Meaningful only when
    /// [`mouse_reporting`](Self::mouse_reporting) is not `None`.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if ghostty rejects the mode query.
    pub fn sgr_mouse(&self) -> Result<bool> {
        self.mode_enabled(MOUSE_FORMAT_SGR)
    }

    /// Whether DECSET 2004 (bracketed paste) is set. When it is, an input
    /// driver wraps pasted text in `ESC [ 200 ~` / `ESC [ 201 ~` so the
    /// application (shells, editors) can treat the paste atomically instead
    /// of executing embedded newlines.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if ghostty rejects the mode query.
    pub fn bracketed_paste(&self) -> Result<bool> {
        self.mode_enabled(BRACKETED_PASTE)
    }

    /// Whether DECSET 1004 (focus event reporting) is set. When it is, an
    /// input driver sends `ESC [ I` on focus gain and `ESC [ O` on focus
    /// loss.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if ghostty rejects the mode query.
    pub fn focus_events(&self) -> Result<bool> {
        self.mode_enabled(FOCUS_EVENT)
    }

    /// Whether DEC private mode 2026 (synchronized output) is set. While it
    /// is, a renderer should hold frames: the application is batching screen
    /// updates and will reset the mode when the frame is complete. Pair the
    /// read with a timeout — a crashed application must not freeze the
    /// display.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if ghostty rejects the mode query.
    pub fn synchronized_output(&self) -> Result<bool> {
        self.mode_enabled(SYNCHRONIZED_OUTPUT)
    }

    /// The kitty keyboard protocol flags the application has set
    /// (`CSI = flags ; mode u`, `CSI > flags u`). An empty value means the
    /// legacy encoding applies; any set flag means an input driver must
    /// encode keys per the kitty progressive-enhancement spec.
    ///
    /// # Errors
    /// Returns an [`Error`] if ghostty rejects the state query.
    pub fn kitty_keyboard_flags(&self) -> Result<KittyKeyboardFlags> {
        let bits: u8 = unsafe {
            self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS)
        }?;
        Ok(KittyKeyboardFlags::from_bits(bits))
    }

    /// The OSC 133 semantic-prompt state of one row, addressed in any of
    /// ghostty's coordinate spaces. [`RowLocation::Screen`] covers the
    /// scrollback, so a caller can walk prompt marks across history (e.g.
    /// jump-to-previous-prompt).
    ///
    /// Resolving an arbitrary row is not free (ghostty documents grid
    /// references as unfit for per-frame render loops); query on demand, not
    /// per rendered frame across the whole scrollback.
    ///
    /// # Errors
    /// Returns [`Error::InvalidValue`] if the row is out of range for its
    /// coordinate space, or another [`Error`] if ghostty rejects the query.
    pub fn row_semantic_prompt(&self, row: RowLocation) -> Result<RowSemanticPrompt> {
        // A grid ref is a "sized" struct: `size` must be set before the call
        // so the library can detect the caller's struct layout.
        let mut grid_ref: sys::GhosttyGridRef = unsafe { std::mem::zeroed() };
        grid_ref.size = std::mem::size_of::<sys::GhosttyGridRef>();
        check(unsafe { sys::ghostty_terminal_grid_ref(self.raw, row.point(), &raw mut grid_ref) })?;
        let mut raw_row: sys::GhosttyRow = 0;
        check(unsafe { sys::ghostty_grid_ref_row(&raw const grid_ref, &raw mut raw_row) })?;
        // Read the enum as its raw u32 and map known values instead of
        // materializing the C enum from FFI output (an out-of-range tag
        // would be UB to form as a Rust enum).
        let mut semantic: u32 = 0;
        check(unsafe {
            sys::ghostty_row_get(
                raw_row,
                sys::GhosttyRowData::GHOSTTY_ROW_DATA_SEMANTIC_PROMPT,
                (&raw mut semantic).cast::<c_void>(),
            )
        })?;
        Ok(match semantic {
            1 => RowSemanticPrompt::Prompt,
            2 => RowSemanticPrompt::PromptContinuation,
            _ => RowSemanticPrompt::None,
        })
    }

    /// Dump the terminal's full text — scrollback plus the active screen —
    /// as plain text, with soft-wrapped lines joined and trailing whitespace
    /// trimmed (select-all-copy semantics). Escape sequences and styling are
    /// not included.
    ///
    /// # Errors
    /// Returns an [`Error`] if ghostty cannot allocate the formatter or
    /// rejects a format call.
    pub fn dump_text(&self) -> Result<String> {
        // The extra structs are "sized" like GhosttyStyle: their `size`
        // fields must be set even with every extra disabled.
        let mut options: sys::GhosttyFormatterTerminalOptions = unsafe { std::mem::zeroed() };
        options.size = std::mem::size_of::<sys::GhosttyFormatterTerminalOptions>();
        options.emit = sys::GhosttyFormatterFormat::GHOSTTY_FORMATTER_FORMAT_PLAIN;
        options.unwrap = true;
        options.trim = true;
        options.extra.size = std::mem::size_of::<sys::GhosttyFormatterTerminalExtra>();
        options.extra.screen.size = std::mem::size_of::<sys::GhosttyFormatterScreenExtra>();

        let formatter = Formatter::new(self, options)?;

        // Size query: NULL buffer returns OUT_OF_SPACE with the required
        // size (or SUCCESS when there is nothing to emit).
        let mut needed: usize = 0;
        let query = unsafe {
            sys::ghostty_formatter_format_buf(formatter.raw, ptr::null_mut(), 0, &raw mut needed)
        };
        match query {
            sys::GhosttyResult::GHOSTTY_SUCCESS => return Ok(String::new()),
            sys::GhosttyResult::GHOSTTY_OUT_OF_SPACE => {}
            other => return Err(check(other).unwrap_err()),
        }

        let mut buf = vec![0u8; needed];
        let mut written: usize = 0;
        check(unsafe {
            sys::ghostty_formatter_format_buf(
                formatter.raw,
                buf.as_mut_ptr(),
                buf.len(),
                &raw mut written,
            )
        })?;
        buf.truncate(written);
        // Ghostty stores codepoints, so the dump is UTF-8; lossy conversion
        // is a no-op safety net rather than an expected path.
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Whether the alternate screen is active (DECSET 47/1047/1049), read
    /// from the terminal's active-screen state rather than any single mode
    /// bit, so every enter/leave path is covered.
    ///
    /// # Errors
    /// Returns an [`Error`] if ghostty rejects the state query.
    pub fn alternate_screen(&self) -> Result<bool> {
        let screen: sys::GhosttyTerminalScreen =
            unsafe { self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN) }?;
        Ok(screen == sys::GhosttyTerminalScreen::GHOSTTY_TERMINAL_SCREEN_ALTERNATE)
    }

    /// Scrollbar geometry of the current viewport (ghostty's terminal
    /// scrollbar data). Ghostty documents this query as potentially
    /// expensive when the viewport is pinned into scrollback; call it per
    /// rendered frame, not per byte fed.
    ///
    /// # Errors
    /// Returns an [`Error`] if ghostty rejects the state query.
    pub fn scrollbar(&self) -> Result<Scrollbar> {
        let bar: sys::GhosttyTerminalScrollbar =
            unsafe { self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_SCROLLBAR) }?;
        Ok(Scrollbar {
            total: bar.total,
            offset: bar.offset,
            len: bar.len,
        })
    }

    /// Read a DEC/ANSI mode's current state via `ghostty_terminal_mode_get`.
    fn mode_enabled(&self, mode: sys::GhosttyMode) -> Result<bool> {
        let mut out = false;
        check(unsafe { sys::ghostty_terminal_mode_get(self.raw, mode, &raw mut out) })?;
        Ok(out)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { sys::ghostty_terminal_free(self.raw) };
    }
}

/// Owned wrapper over a `GhosttyFormatter`, freed on drop. The formatter
/// borrows the terminal (the C API stores a reference), so the lifetime ties
/// it to the [`Terminal`] it formats.
struct Formatter<'terminal> {
    raw: sys::GhosttyFormatter,
    _terminal: std::marker::PhantomData<&'terminal Terminal>,
}

impl<'terminal> Formatter<'terminal> {
    fn new(
        terminal: &'terminal Terminal,
        options: sys::GhosttyFormatterTerminalOptions,
    ) -> Result<Self> {
        let mut raw: sys::GhosttyFormatter = ptr::null_mut();
        check(unsafe {
            sys::ghostty_formatter_terminal_new(ptr::null(), &raw mut raw, terminal.raw, options)
        })?;
        Ok(Self {
            raw,
            _terminal: std::marker::PhantomData,
        })
    }
}

impl Drop for Formatter<'_> {
    fn drop(&mut self) {
        unsafe { sys::ghostty_formatter_free(self.raw) };
    }
}

/// Owned wrapper over a `GhosttyRenderState`, freed on drop.
struct RenderState {
    raw: sys::GhosttyRenderState,
}

impl RenderState {
    fn new() -> Result<Self> {
        let mut raw: sys::GhosttyRenderState = ptr::null_mut();
        check(unsafe { sys::ghostty_render_state_new(ptr::null(), &raw mut raw) })?;
        Ok(Self { raw })
    }

    /// Read a scalar value of type `T` from the render state.
    ///
    /// # Safety
    /// `T` must match the C output type documented for `data`.
    unsafe fn get<T>(&self, data: sys::GhosttyRenderStateData) -> Result<T> {
        let mut out = std::mem::MaybeUninit::<T>::uninit();
        check(unsafe {
            sys::ghostty_render_state_get(self.raw, data, out.as_mut_ptr().cast::<c_void>())
        })?;
        Ok(unsafe { out.assume_init() })
    }

    /// Read the cursor state out of the render state.
    fn cursor(&self) -> Result<Cursor> {
        use sys::GhosttyRenderStateData as Data;

        let visual_style: sys::GhosttyRenderStateCursorVisualStyle =
            unsafe { self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE) }?;
        let visible: bool = unsafe { self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE) }?;
        let blinking: bool = unsafe { self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING) }?;

        let has_viewport: bool =
            unsafe { self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE) }?;
        let viewport = if has_viewport {
            let x: u16 = unsafe { self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X) }?;
            let y: u16 = unsafe { self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y) }?;
            Some((x, y))
        } else {
            None
        };

        Ok(Cursor {
            visible,
            blinking,
            visual_style: visual_style.into(),
            viewport,
        })
    }

    /// Read the full viewport as owned [`Row`]s.
    fn viewport(&self, cols: u16) -> Result<Vec<Row>> {
        let mut iterator = RowIterator::new()?;
        check(unsafe {
            sys::ghostty_render_state_get(
                self.raw,
                sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut iterator.raw).cast::<c_void>(),
            )
        })?;

        let mut cells = RowCells::new()?;
        let mut viewport = Vec::new();
        while unsafe { sys::ghostty_render_state_row_iterator_next(iterator.raw) } {
            let wrapped = read_wrapped(&iterator)?;
            check(unsafe {
                sys::ghostty_render_state_row_get(
                    iterator.raw,
                    sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&raw mut cells.raw).cast::<c_void>(),
                )
            })?;
            viewport.push(Row {
                cells: read_row(&cells, cols)?,
                wrapped,
            });
        }
        Ok(viewport)
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        unsafe { sys::ghostty_render_state_free(self.raw) };
    }
}

/// Owned wrapper over a `GhosttyRenderStateRowIterator`, freed on drop.
struct RowIterator {
    raw: sys::GhosttyRenderStateRowIterator,
}

impl RowIterator {
    fn new() -> Result<Self> {
        let mut raw: sys::GhosttyRenderStateRowIterator = ptr::null_mut();
        check(unsafe { sys::ghostty_render_state_row_iterator_new(ptr::null(), &raw mut raw) })?;
        Ok(Self { raw })
    }
}

impl Drop for RowIterator {
    fn drop(&mut self) {
        unsafe { sys::ghostty_render_state_row_iterator_free(self.raw) };
    }
}

/// Owned wrapper over a `GhosttyRenderStateRowCells`, freed on drop.
struct RowCells {
    raw: sys::GhosttyRenderStateRowCells,
}

impl RowCells {
    fn new() -> Result<Self> {
        let mut raw: sys::GhosttyRenderStateRowCells = ptr::null_mut();
        check(unsafe { sys::ghostty_render_state_row_cells_new(ptr::null(), &raw mut raw) })?;
        Ok(Self { raw })
    }

    /// Read a scalar value of type `T` for the currently selected cell.
    ///
    /// # Safety
    /// A cell must be selected via `ghostty_render_state_row_cells_select`, and
    /// `T` must match the C output type documented for `data`.
    unsafe fn get<T>(&self, data: sys::GhosttyRenderStateRowCellsData) -> Result<T> {
        let mut out = std::mem::MaybeUninit::<T>::uninit();
        check(unsafe {
            sys::ghostty_render_state_row_cells_get(
                self.raw,
                data,
                out.as_mut_ptr().cast::<c_void>(),
            )
        })?;
        Ok(unsafe { out.assume_init() })
    }
}

impl Drop for RowCells {
    fn drop(&mut self) {
        unsafe { sys::ghostty_render_state_row_cells_free(self.raw) };
    }
}

/// Whether the iterator's current row is soft-wrapped onto the next one.
///
/// Two hops, because the render state and the screen are two surfaces over
/// the same row: the iterator hands back the raw `GhosttyRow` handle, and the
/// wrap bit lives on the screen side with the rest of the row flags. The
/// render state's own row data is only dirty/raw/cells.
fn read_wrapped(iterator: &RowIterator) -> Result<bool> {
    let mut raw_row: sys::GhosttyRow = 0;
    check(unsafe {
        sys::ghostty_render_state_row_get(
            iterator.raw,
            sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
            (&raw mut raw_row).cast::<c_void>(),
        )
    })?;
    let mut wrapped = false;
    check(unsafe {
        sys::ghostty_row_get(
            raw_row,
            sys::GhosttyRowData::GHOSTTY_ROW_DATA_WRAP,
            (&raw mut wrapped).cast::<c_void>(),
        )
    })?;
    Ok(wrapped)
}

/// The selected cell's [`CellWide`].
///
/// Same two hops as [`read_wrapped`] and for the same reason: the wide tag is
/// a screen-side cell property, and the render state's cell view only reaches
/// it through the raw handle.
fn read_wide(cells: &RowCells) -> Result<CellWide> {
    let raw_cell: sys::GhosttyCell =
        unsafe { cells.get(sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW) }?;
    let mut wide: u32 = 0;
    check(unsafe {
        sys::ghostty_cell_get(
            raw_cell,
            sys::GhosttyCellData::GHOSTTY_CELL_DATA_WIDE,
            (&raw mut wide).cast::<c_void>(),
        )
    })?;
    Ok(CellWide::from_raw(wide))
}

/// Read every cell of the selected row into owned [`Cell`]s.
fn read_row(cells: &RowCells, cols: u16) -> Result<Vec<Cell>> {
    use sys::GhosttyRenderStateRowCellsData as CellData;

    let mut row = Vec::with_capacity(cols as usize);
    for col in 0..cols {
        check(unsafe { sys::ghostty_render_state_row_cells_select(cells.raw, col) })?;

        // The style struct is "sized": its `size` field must be set before the
        // call so the library can detect the caller's struct layout.
        let mut style_raw: sys::GhosttyStyle = unsafe { std::mem::zeroed() };
        style_raw.size = std::mem::size_of::<sys::GhosttyStyle>();
        check(unsafe {
            sys::ghostty_render_state_row_cells_get(
                cells.raw,
                CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                (&raw mut style_raw).cast::<c_void>(),
            )
        })?;
        let style = unsafe { Style::from_raw(&style_raw) };

        let Grapheme {
            base: ch,
            combining,
        } = read_graphemes(cells)?;

        // Resolved colors return GHOSTTY_INVALID_VALUE when the cell has no
        // explicit color; that is the documented "use your default" signal, not
        // a hard error, so map it to None.
        let fg = read_resolved_color(
            cells,
            CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
        )?;
        let bg = read_resolved_color(
            cells,
            CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
        )?;

        let hyperlink = read_hyperlink(cells)?;
        let wide = read_wide(cells)?;

        row.push(Cell {
            ch,
            combining,
            style,
            fg,
            bg,
            hyperlink,
            wide,
        });
    }
    Ok(row)
}

/// Read the selected cell's OSC 8 hyperlink URI, `None` when it has none.
///
/// Two-call convention like [`read_graphemes`]: a length query sizes the
/// caller-provided buffer the URI bytes are copied into. A zero length is
/// the library's "no hyperlink" signal (it also covers a set hyperlink flag
/// whose id resolved to no entry, which the C API reports as an empty URI).
fn read_hyperlink(cells: &RowCells) -> Result<Option<String>> {
    use sys::GhosttyRenderStateRowCellsData as CellData;

    let len: u32 =
        unsafe { cells.get(CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HYPERLINK_URI_LEN) }?;
    if len == 0 {
        return Ok(None);
    }

    let mut buf = vec![0u8; len as usize];
    check(unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells.raw,
            CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HYPERLINK_URI_BUF,
            buf.as_mut_ptr().cast::<c_void>(),
        )
    })?;

    // Ghostty stores the URI bytes as the application sent them; OSC 8
    // payloads are ASCII-restricted in practice but not enforced, so decode
    // lossily rather than erroring the whole row on a weird sequence.
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// A cell's grapheme: its base codepoint plus any trailing combining marks.
struct Grapheme {
    base: Option<char>,
    combining: Vec<char>,
}

/// Read the base codepoint plus any combining marks of the selected cell.
fn read_graphemes(cells: &RowCells) -> Result<Grapheme> {
    use sys::GhosttyRenderStateRowCellsData as CellData;

    let len: u32 =
        unsafe { cells.get(CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN) }?;
    if len == 0 {
        return Ok(Grapheme {
            base: None,
            combining: Vec::new(),
        });
    }

    let mut buf = vec![0u32; len as usize];
    check(unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells.raw,
            CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
            buf.as_mut_ptr().cast::<c_void>(),
        )
    })?;

    let mut codepoints = buf.into_iter().map(char::from_u32);
    let base = codepoints.next().flatten();
    let combining = codepoints.flatten().collect();
    Ok(Grapheme { base, combining })
}

/// Read a resolved cell color, mapping the "no explicit color" signal to `None`.
fn read_resolved_color(
    cells: &RowCells,
    data: sys::GhosttyRenderStateRowCellsData,
) -> Result<Option<Rgb>> {
    let mut out = std::mem::MaybeUninit::<sys::GhosttyColorRgb>::uninit();
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get(cells.raw, data, out.as_mut_ptr().cast::<c_void>())
    };
    match result {
        sys::GhosttyResult::GHOSTTY_SUCCESS => Ok(Some(unsafe { out.assume_init() }.into())),
        sys::GhosttyResult::GHOSTTY_INVALID_VALUE => Ok(None),
        other => Err(check(other).unwrap_err()),
    }
}

/// `ESC` (0x1b), the introducer of every escape sequence.
const ESC: u8 = 0x1b;
/// `BEL` (0x07), one of the two OSC terminators.
const BEL: u8 = 0x07;
/// `]` (0x5d): the byte after `ESC` that opens an OSC sequence.
const OSC_INTRODUCER: u8 = b']';
/// `\` (0x5c): the byte after `ESC` that forms a String Terminator (ST).
const ST_FINAL: u8 = b'\\';
/// `CAN` (0x18) and `SUB` (0x1a): per ECMA-48 either one aborts an in-progress
/// control string, so they cancel a partial OSC rather than landing in its text.
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

/// Where the framing scanner is within an `ESC ] … (BEL | ESC | CAN | SUB)` OSC
/// sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameState {
    /// Outside any escape sequence.
    Ground,
    /// Saw `ESC`; a following `]` opens an OSC (and a following `\` after an OSC
    /// is the tail of a String Terminator, which lands back in `Ground`).
    Escape,
    /// Inside an OSC payload, forwarding bytes to the parser.
    Osc,
}

/// Tracks the most recent OSC window title in a raw terminal byte stream.
///
/// libghostty-vt's [`Terminal`] does not expose the window title an application
/// sets via OSC 0/2 (there is no such `ghostty_terminal_get` key), so a consumer
/// that needs it (e.g. a multiplexer labeling a session by its live title) feeds
/// the same byte stream here. This wraps ghostty's streaming OSC parser behind a
/// small `ESC ]` … `BEL`/`ST` framing scanner, so a title sequence split across
/// reads is still captured. Using ghostty's parser (not a hand-rolled scan) is
/// what makes the OSC 0 vs 1 vs 2 title/icon classification correct.
///
/// The captured [`title`](Self::title) is owned, so it stays valid after the
/// next [`feed`](Self::feed). Like [`Terminal`], the underlying parser has
/// thread affinity (the raw pointer keeps it `!Send`); keep the tracker on one
/// thread and do not add an `unsafe impl Send`.
///
/// Framing covers the 7-bit OSC forms terminal programs actually emit (`ESC ]`
/// opener, `BEL` or `ESC \` terminator, `CAN`/`SUB` abort); the 8-bit C1 forms
/// (`0x9d` opener, `0x9c` ST) are not recognized.
pub struct OscTitleTracker {
    parser: sys::GhosttyOscParser,
    frame: FrameState,
    title: Option<String>,
}

impl OscTitleTracker {
    /// Create an empty tracker.
    ///
    /// # Errors
    /// Returns [`Error::OutOfMemory`] if ghostty cannot allocate the parser.
    pub fn new() -> Result<Self> {
        let mut parser: sys::GhosttyOscParser = ptr::null_mut();
        check(unsafe { sys::ghostty_osc_new(ptr::null(), &raw mut parser) })?;
        Ok(Self {
            parser,
            frame: FrameState::Ground,
            title: None,
        })
    }

    /// The most recent window title seen, if any.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Feed a chunk of raw terminal output, updating [`title`](Self::title) when
    /// a complete OSC title sequence is seen. Byte boundaries are arbitrary: the
    /// framing state persists across calls, so a sequence split mid-stream is
    /// still parsed.
    pub fn feed(&mut self, data: &[u8]) {
        for &byte in data {
            self.frame = self.step(self.frame, byte);
        }
    }

    /// Advance the framing scanner one byte from `state`, returning the next
    /// state and driving the OSC parser for payload bytes / terminators.
    fn step(&mut self, state: FrameState, byte: u8) -> FrameState {
        match state {
            FrameState::Ground if byte == ESC => FrameState::Escape,
            FrameState::Ground => FrameState::Ground,
            FrameState::Escape => match byte {
                OSC_INTRODUCER => {
                    // Reset on open so a previous OSC abandoned without a
                    // terminator (e.g. an `ESC [` arrived mid-payload) cannot
                    // bleed into this one.
                    unsafe { sys::ghostty_osc_reset(self.parser) };
                    FrameState::Osc
                }
                // Consecutive ESC: still waiting on the introducer.
                ESC => FrameState::Escape,
                _ => FrameState::Ground,
            },
            FrameState::Osc => match byte {
                BEL => {
                    self.finish(BEL);
                    FrameState::Ground
                }
                // CAN/SUB abort the control string: drop the partial OSC.
                CAN | SUB => {
                    unsafe { sys::ghostty_osc_reset(self.parser) };
                    FrameState::Ground
                }
                // ESC always ends the OSC string: a following `\` makes it a
                // clean ST, but any other escape sequence still terminates and
                // dispatches the OSC (ghostty does the same). So finish here and
                // re-enter Escape; a following `\` then harmlessly falls back to
                // Ground, and a `[`, `]`, … starts its own sequence.
                ESC => {
                    self.finish(ST_FINAL);
                    FrameState::Escape
                }
                _ => {
                    unsafe { sys::ghostty_osc_next(self.parser, byte) };
                    FrameState::Osc
                }
            },
        }
    }

    /// Terminate the current OSC and, if it set the window title, capture it.
    fn finish(&mut self, terminator: u8) {
        let command = unsafe { sys::ghostty_osc_end(self.parser, terminator) };
        // Ask only for the title string rather than reading the command *type*:
        // `ghostty_osc_command_data` returns false for any command that is not a
        // window-title change, which both classifies the command and avoids
        // materializing the `GhosttyOscCommandType` enum from the FFI return
        // (ghostty could in principle return a tag outside the checked-in enum,
        // which would be UB to form as a Rust enum).
        let mut text: *const c_char = ptr::null();
        let ok = unsafe {
            sys::ghostty_osc_command_data(
                command,
                sys::GhosttyOscCommandData::GHOSTTY_OSC_DATA_CHANGE_WINDOW_TITLE_STR,
                (&raw mut text).cast::<c_void>(),
            )
        };
        // The string is owned by the parser and only valid until the next
        // `ghostty_osc_*` call (including the reset below), so copy it now. Skip
        // a title that is not valid UTF-8 (ghostty's stream path ignores those
        // too) rather than substituting replacement characters.
        if ok
            && !text.is_null()
            && let Ok(title) = unsafe { CStr::from_ptr(text) }.to_str()
        {
            self.title = Some(title.to_owned());
        }
        unsafe { sys::ghostty_osc_reset(self.parser) };
    }
}

impl Drop for OscTitleTracker {
    fn drop(&mut self) {
        unsafe { sys::ghostty_osc_free(self.parser) };
    }
}
