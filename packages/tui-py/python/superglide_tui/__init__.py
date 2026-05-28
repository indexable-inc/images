"""Python bindings for the `tui` Rust crate.

Spawn and control multiple pseudo-terminal (PTY) backed processes from Python,
with full vt100 emulation, scrollback, and optional NumPy access to per-cell
state.
"""

from __future__ import annotations

from ._superglide_tui import (
    FullOutput,
    StyledCell,
    TuiInstance,
    TuiManager,
    __version__,
)

__all__ = [
    "FullOutput",
    "StyledCell",
    "TuiInstance",
    "TuiManager",
    "__version__",
]
