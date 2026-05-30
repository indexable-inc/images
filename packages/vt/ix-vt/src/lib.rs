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

use std::ffi::c_void;
use std::fmt;
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
    /// A result code outside the documented enum.
    Unknown(i32),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => f.write_str("libghostty-vt: out of memory"),
            Self::InvalidValue => f.write_str("libghostty-vt: invalid value"),
            Self::OutOfSpace => f.write_str("libghostty-vt: out of space"),
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
            sys::GhosttyStyleColorTag::GHOSTTY_STYLE_COLOR_NONE => Self::None,
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
            underline: (raw.underline != 0).then(|| {
                u8::try_from(raw.underline).unwrap_or(u8::MAX)
            }),
            fg_color: unsafe { StyleColor::from_raw(raw.fg_color) },
            bg_color: unsafe { StyleColor::from_raw(raw.bg_color) },
            underline_color: unsafe { StyleColor::from_raw(raw.underline_color) },
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
            Raw::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => Self::Block,
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
    pub viewport: Vec<Vec<Cell>>,
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

/// Options for creating a [`Terminal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    /// Number of columns (width in cells).
    pub cols: u16,
    /// Number of rows (height in cells).
    pub rows: u16,
    /// Maximum number of lines to keep in scrollback history.
    pub max_scrollback: usize,
}

/// A terminal VT engine instance.
///
/// Owns the underlying `GhosttyTerminal` and frees it on drop.
pub struct Terminal {
    raw: sys::GhosttyTerminal_ptr,
}

// `Terminal` is intentionally left `!Send` and `!Sync` (the raw pointer makes it
// so by default). libghostty-vt's terminal has thread affinity, so the handle
// must stay on the thread that created it; a caller that needs it from async or
// another thread owns it on a pinned thread behind a channel API rather than
// moving the handle. Do not add an `unsafe impl Send`/`Sync`.

impl Terminal {
    /// Create a terminal sized `rows` by `cols` with `scrollback` lines of
    /// history.
    ///
    /// The argument order is `(rows, cols, scrollback)` to read like a screen
    /// size; the underlying C struct stores `cols`/`rows` separately, so there
    /// is no ambiguity once constructed.
    ///
    /// # Errors
    /// Returns an [`Error`] if ghostty cannot allocate the terminal (see
    /// [`Self::with_options`]).
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Result<Self> {
        Self::with_options(TerminalOptions {
            cols,
            rows,
            max_scrollback: scrollback,
        })
    }

    /// Create a terminal from explicit [`TerminalOptions`].
    ///
    /// # Errors
    /// Returns [`Error::OutOfMemory`] if ghostty cannot allocate the terminal,
    /// or [`Error::InvalidValue`] if it rejects the options.
    pub fn with_options(options: TerminalOptions) -> Result<Self> {
        let mut raw: sys::GhosttyTerminal_ptr = ptr::null_mut();
        let opts = sys::GhosttyTerminalOptions {
            cols: options.cols,
            rows: options.rows,
            max_scrollback: options.max_scrollback,
        };
        // Passing a null allocator selects the default (libc malloc/free).
        check(unsafe { sys::ghostty_terminal_new(ptr::null(), &raw mut raw, opts) })?;
        Ok(Self { raw })
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
        check(unsafe { sys::ghostty_terminal_resize(self.raw, cols, rows) })
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
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { sys::ghostty_terminal_free(self.raw) };
    }
}

/// Owned wrapper over a `GhosttyRenderState`, freed on drop.
struct RenderState {
    raw: sys::GhosttyRenderState_ptr,
}

impl RenderState {
    fn new() -> Result<Self> {
        let mut raw: sys::GhosttyRenderState_ptr = ptr::null_mut();
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

    /// Read the full viewport as owned rows of [`Cell`]s.
    fn viewport(&self, cols: u16) -> Result<Vec<Vec<Cell>>> {
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
            check(unsafe {
                sys::ghostty_render_state_row_get(
                    iterator.raw,
                    sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&raw mut cells.raw).cast::<c_void>(),
                )
            })?;
            viewport.push(read_row(&cells, cols)?);
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
    raw: sys::GhosttyRenderStateRowIterator_ptr,
}

impl RowIterator {
    fn new() -> Result<Self> {
        let mut raw: sys::GhosttyRenderStateRowIterator_ptr = ptr::null_mut();
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
    raw: sys::GhosttyRenderStateRowCells_ptr,
}

impl RowCells {
    fn new() -> Result<Self> {
        let mut raw: sys::GhosttyRenderStateRowCells_ptr = ptr::null_mut();
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

        let (ch, combining) = read_graphemes(cells)?;

        // Resolved colors return GHOSTTY_INVALID_VALUE when the cell has no
        // explicit color; that is the documented "use your default" signal, not
        // a hard error, so map it to None.
        let fg = read_resolved_color(cells, CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR)?;
        let bg = read_resolved_color(cells, CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR)?;

        row.push(Cell {
            ch,
            combining,
            style,
            fg,
            bg,
        });
    }
    Ok(row)
}

/// Read the base codepoint plus any combining marks of the selected cell.
fn read_graphemes(cells: &RowCells) -> Result<(Option<char>, Vec<char>)> {
    use sys::GhosttyRenderStateRowCellsData as CellData;

    let len: u32 =
        unsafe { cells.get(CellData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN) }?;
    if len == 0 {
        return Ok((None, Vec::new()));
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
    Ok((base, combining))
}

/// Read a resolved cell color, mapping the "no explicit color" signal to `None`.
fn read_resolved_color(
    cells: &RowCells,
    data: sys::GhosttyRenderStateRowCellsData,
) -> Result<Option<Rgb>> {
    let mut out = std::mem::MaybeUninit::<sys::GhosttyColorRgb>::uninit();
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells.raw,
            data,
            out.as_mut_ptr().cast::<c_void>(),
        )
    };
    match result {
        sys::GhosttyResult::GHOSTTY_SUCCESS => Ok(Some(unsafe { out.assume_init() }.into())),
        sys::GhosttyResult::GHOSTTY_INVALID_VALUE => Ok(None),
        other => Err(check(other).unwrap_err()),
    }
}
