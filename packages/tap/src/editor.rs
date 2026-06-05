//! Open the session scrollback in the user's editor.
//!
//! The keybind drops the terminal back to cooked mode (via [`RawGuard`]), writes
//! the scrollback to a temp file, runs the editor at the cursor line when the
//! editor's argument syntax is known, then re-enters raw mode. The attach loop
//! repaints afterward.

use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::term::RawGuard;

/// Editors whose line/column argument syntax we know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    /// vim, nvim, vi: `+{line}` before the file.
    Vim,
    /// VS Code, Cursor: `-g {file}:{line}:{col}`.
    VsCode,
    /// nano: `+{line},{col}` before the file.
    Nano,
    /// emacs: `+{line}:{col}` before the file.
    Emacs,
    /// helix: `{file}:{line}`.
    Helix,
    /// Anything else: open the file with no position.
    Unknown,
}

impl EditorKind {
    /// Detect the editor kind from a command name or path.
    #[must_use]
    pub fn detect(cmd: &str) -> Self {
        let name = Path::new(cmd)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd);
        match name {
            "vim" | "nvim" | "vi" | "view" | "vimdiff" => Self::Vim,
            "code" | "cursor" | "code-insiders" | "codium" | "vscodium" => Self::VsCode,
            "nano" | "pico" => Self::Nano,
            "emacs" | "emacsclient" => Self::Emacs,
            "hx" | "helix" => Self::Helix,
            _ => Self::Unknown,
        }
    }
}

/// A 1-indexed file position.
#[derive(Debug, Clone, Copy)]
pub struct Position {
    /// 1-indexed line.
    pub line: usize,
    /// 1-indexed column, if known.
    pub col: Option<usize>,
}

/// The argument vector for invoking an editor at a position: the flags that
/// precede the file, plus the file argument itself (which may carry the position).
pub struct EditorArgs {
    /// Arguments placed before the file argument (e.g. `+42`, `-g`).
    pub pos_args: Vec<String>,
    /// The file argument, possibly suffixed with `:line:col`.
    pub file_arg: String,
}

/// Build the argument vector for opening `file_path` at `pos`.
#[must_use]
pub fn build_args(editor_cmd: &str, file_path: &Path, pos: Option<Position>) -> EditorArgs {
    let file = file_path.display().to_string();
    let Some(pos) = pos else {
        return EditorArgs {
            pos_args: vec![],
            file_arg: file,
        };
    };
    let (pos_args, file_arg) = match EditorKind::detect(editor_cmd) {
        EditorKind::Vim => (vec![format!("+{}", pos.line)], file),
        EditorKind::VsCode => {
            let col = pos.col.unwrap_or(1);
            (vec!["-g".to_string()], format!("{file}:{}:{col}", pos.line))
        }
        EditorKind::Nano => {
            let arg = pos
                .col
                .map_or_else(|| format!("+{}", pos.line), |col| format!("+{},{col}", pos.line));
            (vec![arg], file)
        }
        EditorKind::Emacs => {
            let arg = pos
                .col
                .map_or_else(|| format!("+{}", pos.line), |col| format!("+{}:{col}", pos.line));
            (vec![arg], file)
        }
        EditorKind::Helix => (vec![], format!("{file}:{}", pos.line)),
        EditorKind::Unknown => (vec![], file),
    };
    EditorArgs { pos_args, file_arg }
}

/// Open `content` in `editor_cmd`, restoring raw mode afterward.
///
/// # Errors
///
/// Returns an error if the temp file cannot be written or the editor cannot be
/// spawned. Raw mode is restored on every path.
pub fn open(content: &str, editor_cmd: &str, raw: &RawGuard, pos: Option<Position>) -> Result<()> {
    use std::io::Write as _;

    let mut temp = tempfile::Builder::new()
        .prefix("tap-scrollback-")
        .suffix(".txt")
        .tempfile()
        .context("creating scrollback temp file")?;
    temp.write_all(content.as_bytes())
        .context("writing scrollback temp file")?;
    temp.flush().context("flushing scrollback temp file")?;

    let parts: Vec<&str> = editor_cmd.split_whitespace().collect();
    let (program, leading) = parts
        .split_first()
        .context("empty editor command; set $EDITOR or configure tap")?;
    let EditorArgs { pos_args, file_arg } = build_args(program, temp.path(), pos);

    // Hand the tty to the editor in cooked mode, then take it back.
    raw.suspend();
    let status = std::process::Command::new(program)
        .args(leading)
        .args(pos_args)
        .arg(&file_arg)
        .status();
    raw.resume();

    let status = status.with_context(|| format!("spawning editor '{program}'"))?;
    if !status.success() {
        bail!("editor '{program}' exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_editors() {
        assert_eq!(EditorKind::detect("/usr/bin/nvim"), EditorKind::Vim);
        assert_eq!(EditorKind::detect("cursor"), EditorKind::VsCode);
        assert_eq!(EditorKind::detect("hx"), EditorKind::Helix);
        assert_eq!(EditorKind::detect("mystery"), EditorKind::Unknown);
    }

    #[test]
    fn builds_position_args_per_editor() {
        let EditorArgs { pos_args, file_arg } =
            build_args("vim", Path::new("/t.txt"), Some(Position { line: 42, col: None }));
        assert_eq!(pos_args, vec!["+42"]);
        assert_eq!(file_arg, "/t.txt");

        let EditorArgs { pos_args, file_arg } = build_args(
            "cursor",
            Path::new("/t.txt"),
            Some(Position { line: 42, col: Some(10) }),
        );
        assert_eq!(pos_args, vec!["-g"]);
        assert_eq!(file_arg, "/t.txt:42:10");
    }
}
