use pyo3::prelude::*;

/// A single styled terminal cell: character plus VT100 attributes.
#[pyclass(frozen, get_all, skip_from_py_object, module = "tui._tui")]
#[derive(Debug, Clone)]
pub struct StyledCell {
    pub character: String,
    pub fgcolor: Option<String>,
    pub bgcolor: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[pymethods]
impl StyledCell {
    fn __repr__(&self) -> String {
        format!(
            "StyledCell(character={:?}, fgcolor={:?}, bgcolor={:?}, bold={}, italic={}, \
             underline={}, inverse={})",
            self.character,
            self.fgcolor,
            self.bgcolor,
            self.bold,
            self.italic,
            self.underline,
            self.inverse,
        )
    }
}

impl From<tui::StyledCell> for StyledCell {
    fn from(cell: tui::StyledCell) -> Self {
        Self {
            character: cell.character.to_string(),
            fgcolor: cell.fgcolor,
            bgcolor: cell.bgcolor,
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
            inverse: cell.inverse,
        }
    }
}

/// Combined scrollback and viewport snapshot.
#[pyclass(frozen, get_all, skip_from_py_object, module = "tui._tui")]
#[derive(Debug, Clone)]
pub struct FullOutput {
    pub scrollback: Vec<String>,
    pub viewport: Vec<String>,
}

#[pymethods]
impl FullOutput {
    fn __repr__(&self) -> String {
        format!(
            "FullOutput(scrollback=<{} lines>, viewport=<{} lines>)",
            self.scrollback.len(),
            self.viewport.len(),
        )
    }
}

impl From<tui::FullOutput> for FullOutput {
    fn from(full: tui::FullOutput) -> Self {
        Self {
            scrollback: full.scrollback,
            viewport: full.viewport,
        }
    }
}
