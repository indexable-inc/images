# superglide-tui

Python bindings for the [`tui`](../tui) Rust crate. Spawn and control multiple
PTY-backed processes from Python with full vt100 emulation, scrollback, and
optional NumPy access to per-cell character data.

The Python API is instance-centric, uses float-seconds timeouts, and exposes
every blocking I/O method as a native asyncio coroutine via
[pyo3-async-runtimes][pyo3-async-runtimes]. No thread-pool hop is involved on
the async path.

## Build

For now the wheel is built with [maturin]. From this directory:

```sh
pip install maturin
maturin develop --release
```

Or to produce a wheel:

```sh
maturin build --release
```

The long-term path is to assemble the wheel through Nix + `cargo-unit`
instead of maturin; tracked by
[indexable-inc/index#262](https://github.com/indexable-inc/index/issues/262).

## Quick start

```python
import re
from superglide_tui import TuiManager

with TuiManager() as mgr, mgr.spawn("python", "-q") as tui:
    tui.enter("1 + 2")
    snap = tui.wait_for(re.compile(r"^3$", re.MULTILINE), timeout=2.0)
    print(snap.viewport[-3:])
    # ('>>> 1 + 2', '3', '>>> ')
```

`tui.snapshot()` returns a frozen `Snapshot` (viewport, scrollback, size). It
supports `str(snap)`, `"needle" in snap`, and `.text` / `.full_text`.

## Examples

### Drive a REPL and read its output

```python
from superglide_tui import TuiManager

with TuiManager() as mgr, mgr.spawn("python", "-q") as tui:
    tui.enter("1 + 2")
    snap = tui.wait_for("3", timeout=2.0)
    print(snap.text.splitlines()[-2:])
```

### Async: many TUIs in parallel

Every I/O method has an `a`-prefixed coroutine variant. These are real Rust
futures bridged into asyncio — no `to_thread` shim.

```python
import asyncio
from superglide_tui import TuiManager

async def run(cmd: str, *args: str) -> str:
    mgr = TuiManager()
    async with mgr.spawn(cmd, *args) as tui:
        await tui.aenter("printf READY")
        snap = await tui.await_for("READY", timeout=2.0)
        return snap.text

async def main() -> None:
    out = await asyncio.gather(
        run("bash", "--norc", "-i"),
        run("bash", "--norc", "-i"),
        run("bash", "--norc", "-i"),
    )
    for text in out:
        print(text.strip().splitlines()[-1])

asyncio.run(main())
```

### Send keystrokes

`Key` is a `StrEnum` whose members are raw ANSI sequences, so they
concatenate with literal text and pass straight to `Tui.send`:

```python
from superglide_tui import Key, TuiManager

with TuiManager() as mgr, mgr.spawn("less", "/etc/hosts") as tui:
    tui.send(Key.PAGE_DOWN, Key.PAGE_DOWN)
    tui.send(Key.ctrl("g"))          # any Ctrl-letter
    tui.send(Key.alt("x"))           # any Alt-letter
    tui.send("q")
```

### Match with regex or a predicate

`wait_for` accepts a substring, a compiled `re.Pattern`, or a callable that
takes the current `Snapshot`:

```python
import re
from superglide_tui import TuiManager

with TuiManager() as mgr, mgr.spawn("bash", "--norc", "-i") as tui:
    tui.enter("ls /etc | head -3")
    snap = tui.wait_for(re.compile(r"^\S+\.conf$", re.MULTILINE), timeout=2.0)

    # Or a predicate over the whole snapshot:
    snap = tui.wait_for(lambda s: len(s.viewport) >= 5, timeout=1.0)
```

### Per-cell data

`tui.chars()` returns a `numpy.uint32` array of Unicode codepoints, shape
`(rows, cols)`. `tui.styled_cells()` returns a nested list of `StyledCell`
dataclasses (`char`, `fg`, `bg`, `bold`, `italic`, `underline`, `inverse`).

```python
import numpy as np
from superglide_tui import TuiManager

with TuiManager() as mgr, mgr.spawn("bash", "--norc", "-i") as tui:
    tui.enter("printf 'abc'")
    tui.wait_for("abc", timeout=1.0)

    cells = tui.chars()                       # NDArray[uint32], (rows, cols)
    has_a = np.any(cells == ord("a"))
    styled = tui.styled_cells()
    bolds = [(r, c) for r, row in enumerate(styled)
                    for c, cell in enumerate(row) if cell.bold]
```

### Handle timeouts

```python
from superglide_tui import TuiManager, WaitTimeout

with TuiManager() as mgr, mgr.spawn("bash", "--norc", "-i") as tui:
    try:
        tui.wait_for("never gonna happen", timeout=0.25)
    except WaitTimeout as exc:
        print("missed it:", exc)
```

## Public surface

| Name           | Purpose                                                |
| -------------- | ------------------------------------------------------ |
| `TuiManager`   | Spawn and track processes. Context-managed.            |
| `Tui`          | One spawned process. All I/O lives here.               |
| `Snapshot`     | Frozen viewport + scrollback + size.                   |
| `Size`         | `(rows, cols)` dataclass.                              |
| `Key`          | ANSI keystroke constants + `Key.ctrl`/`Key.alt`.       |
| `StyledCell`   | One cell with VT100 attributes.                        |
| `WaitTimeout`  | Raised by `wait_for` / `await_for` on deadline expiry. |

[maturin]: https://www.maturin.rs/
[pyo3-async-runtimes]: https://docs.rs/pyo3-async-runtimes/
