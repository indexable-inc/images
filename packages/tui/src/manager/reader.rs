pub(super) mod cells;
pub(super) mod text;

pub(super) use cells::{read_chars, read_styled_cells};
pub(super) use text::{
    read_full, read_output, read_output_blocking, read_scrollback, read_viewport,
};

pub use text::FullOutput;
