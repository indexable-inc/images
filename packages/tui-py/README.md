# tui (Python)

Python bindings for the [`tui`](../tui) Rust crate. Spawn and control multiple
PTY-backed processes from Python with full vt100 emulation, scrollback, and
optional NumPy access to per-cell character data.

The Python API is a single class, `Tui`. Construct it, drive it, read it. A
single process-wide tokio runtime drives every spawned PTY; every blocking
I/O method has an `a`-prefixed coroutine variant that bridges a real Rust
future into asyncio via [pyo3-async-runtimes][pyo3-async-runtimes] — no
thread-pool hop.

PyPI distribution name: `ix-tui`. Import name: `tui`.

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
from tui import Tui

with Tui("python", "-q") as tui:
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
from tui import Tui

with Tui("python", "-q") as tui:
    tui.enter("1 + 2")
    snap = tui.wait_for("3", timeout=2.0)
    print(snap.text.splitlines()[-2:])
```

### Async: many TUIs in parallel

Every I/O method has an `a`-prefixed coroutine variant. These are real Rust
futures bridged into asyncio — no `to_thread` shim.

```python
import asyncio
from tui import Tui

async def run(cmd: str, *args: str) -> str:
    async with Tui(cmd, *args) as t:
        await t.aenter("printf READY")
        snap = await t.await_for("READY", timeout=2.0)
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
from tui import Key, Tui

with Tui("less", "/etc/hosts") as t:
    t.send(Key.PAGE_DOWN, Key.PAGE_DOWN)
    t.send(Key.ctrl("g"))          # any Ctrl-letter
    t.send(Key.alt("x"))           # any Alt-letter
    t.send("q")
```

### Match with regex or a predicate

`wait_for` accepts a substring, a compiled `re.Pattern`, or a callable that
takes the current `Snapshot`:

```python
import re
from tui import Tui

with Tui("bash", "--norc", "-i") as t:
    t.enter("ls /etc | head -3")
    snap = t.wait_for(re.compile(r"\.conf$", re.MULTILINE), timeout=2.0)
    snap = t.wait_for(lambda s: len(s.viewport) >= 5, timeout=1.0)
```

### Per-cell data

`tui.chars()` returns a `numpy.uint32` array of Unicode codepoints, shape
`(rows, cols)`. `tui.styled_cells()` returns a nested list of `StyledCell`
dataclasses (`char`, `fg`, `bg`, `bold`, `italic`, `underline`, `inverse`).

```python
import numpy as np
from tui import Tui

with Tui("bash", "--norc", "-i") as t:
    t.enter("printf 'AAA-MARKER'")
    t.wait_for("AAA-MARKER", timeout=1.0)

    cells = t.chars()                         # NDArray[uint32], (rows, cols)
    has_a = bool(np.any(cells == ord("A")))
    styled = t.styled_cells()
    bolds = [(r, c) for r, row in enumerate(styled)
                    for c, cell in enumerate(row) if cell.bold]
```

### Handle timeouts

```python
from tui import Tui, WaitTimeout

with Tui("bash", "--norc", "-i") as t:
    try:
        t.wait_for("never gonna happen", timeout=0.25)
    except WaitTimeout as exc:
        print("missed it:", exc)
```

### List live instances

`Tui.list_all()` returns every `Tui` still alive in this process — handy in
tests or REPLs:

```python
from tui import Tui
Tui("bash", "--norc", "-i")
Tui("python", "-q")
print(len(Tui.list_all()))   # 2
```

## Public surface

| Name           | Purpose                                                |
| -------------- | ------------------------------------------------------ |
| `Tui`          | One spawned process. Construct it and you have a PTY.  |
| `Snapshot`     | Frozen viewport + scrollback + size.                   |
| `Size`         | `(rows, cols)` dataclass.                              |
| `Key`          | ANSI keystroke constants + `Key.ctrl`/`Key.alt`.       |
| `StyledCell`   | One cell with VT100 attributes.                        |
| `WaitTimeout`  | Raised by `wait_for` / `await_for` on deadline expiry. |

[maturin]: https://www.maturin.rs/
[pyo3-async-runtimes]: https://docs.rs/pyo3-async-runtimes/
