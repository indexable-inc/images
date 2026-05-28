pub(super) mod cells;
pub(super) mod text;

pub(super) use cells::{read_chars, read_styled_cells};
pub(super) use text::{
    read_full, read_output, read_output_blocking, read_scrollback, read_viewport,
};

pub use cells::{read_chars_async, read_styled_cells_async};
pub use text::FullOutput;
pub use text::{
    read_blocking_async, read_full_async, read_scrollback_async, read_viewport_async,
};
