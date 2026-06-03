# tui (Python)

Python bindings for the [`tui`](../tui) Rust crate. Spawn and control multiple
PTY-backed processes from Python with full vt100 emulation, scrollback, and
optional NumPy access to per-cell character data.

The Python API is a single class, `Tui`. Construct it, drive it, read it. The
API is async-only. A single process-wide tokio runtime drives every spawned
PTY, and every I/O method is a coroutine that bridges a real Rust future into
asyncio via [pyo3-async-runtimes][pyo3-async-runtimes], with no thread-pool hop.
Only construction and the shape accessors (`id`, `command`, `size`, `is_alive`,
`exit_code`) are synchronous.

PyPI distribution name: `ix-tui`. Import name: `tui`.

## Build

The wheel is built by Nix, not maturin. The PyO3 cdylib comes out of the shared
`cargo-unit` workspace graph (the same one the rest of the repo's Rust builds
from) and [`wheel/mkwheel.py`](wheel/mkwheel.py) packages it with the Python
source into a PEP 427 wheel. There is no PEP 517 backend; `pip install .` is not
a supported path.

```sh
nix build .#tui-py     # writes ix_tui-<version>-cp311-abi3-manylinux_2_34_<arch>.whl
```

The wheel is Linux-only, like ix's native SDK wheels: a PyO3 extension cdylib
links cleanly only where a shared object may carry undefined symbols (Linux);
macOS needs `-undefined dynamic_lookup`, which the shared cargo-unit graph does
not thread through. From a macOS checkout, build it on a Linux builder with
`nix build .#packages.x86_64-linux.tui-py`. The extension is abi3
(`pyo3/abi3-py311`), so one wheel loads on CPython 3.11+; `pip install` it from
`result/`.

## Quick start

```python
import asyncio
import re
from tui import Tui

async def main() -> None:
    async with Tui("python", "-q") as tui:
        await tui.enter("1 + 2")
        snap = await tui.wait_for(re.compile(r"^3$", re.MULTILINE), timeout=2.0)
        print(snap.viewport[-3:])
        # ('>>> 1 + 2', '3', '>>> ')

asyncio.run(main())
```

`await tui.snapshot()` returns a frozen `Snapshot` (viewport, scrollback, size,
and per-cell `styled`). It supports `str(snap)`, `"needle" in snap`, and
`.text` / `.full_text`. In Jupyter, evaluating a snapshot renders the screen in
color (see [Jupyter notebooks](#jupyter-notebooks)).

The terminal opens at 80x24 with 10,000 lines of scrollback. Override per
instance with keyword args on the (synchronous) constructor:

```python
Tui("bash", "--norc", "-i", rows=40, cols=120, scrollback_lines=50_000)
```

## Examples

### Drive a REPL and read its output

```python
import asyncio
from tui import Tui

async def main() -> None:
    async with Tui("python", "-q") as tui:
        await tui.enter("1 + 2")
        snap = await tui.wait_for("3", timeout=2.0)
        print(snap.text.splitlines()[-2:])

asyncio.run(main())
```

### Many TUIs in parallel

Because every method is a coroutine, fanning out is plain `asyncio.gather`. The
coroutines are real Rust futures bridged into asyncio, so there is no
`to_thread` shim.

```python
import asyncio
from tui import Tui

async def run(cmd: str, *args: str) -> str:
    async with Tui(cmd, *args) as t:
        await t.enter("printf READY")
        snap = await t.wait_for("READY", timeout=2.0)
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
import asyncio
from tui import Key, Tui

async def main() -> None:
    async with Tui("less", "/etc/hosts") as t:
        await t.send(Key.PAGE_DOWN, Key.PAGE_DOWN)
        await t.send(Key.ctrl("g"))          # any Ctrl-letter
        await t.send(Key.alt("x"))           # any Alt-letter
        await t.send("q")

asyncio.run(main())
```

### Match with regex or a predicate

`wait_for` accepts a substring, a compiled `re.Pattern`, or a callable that
takes the current `Snapshot`:

```python
import asyncio
import re
from tui import Tui

async def main() -> None:
    async with Tui("bash", "--norc", "-i") as t:
        await t.enter("ls /etc | head -3")
        snap = await t.wait_for(re.compile(r"\.conf$", re.MULTILINE), timeout=2.0)
        snap = await t.wait_for(lambda s: len(s.viewport) >= 5, timeout=1.0)

asyncio.run(main())
```

### Per-cell data

`await tui.chars()` returns a `numpy.uint32` array of Unicode codepoints, shape
`(rows, cols)`. `await tui.styled_cells()` returns a nested list of `StyledCell`
objects (`char`, `fg`, `bg`, `bold`, `italic`, `underline`, `inverse`).

`fg` and `bg` are `Color` values: `None` for the terminal default, an `int` in
`0..=255` for a palette index, or an `(r, g, b)` tuple for truecolor.

```python
import asyncio
import numpy as np
from tui import Tui

async def main() -> None:
    async with Tui("bash", "--norc", "-i") as t:
        await t.enter("printf 'AAA-MARKER'")
        await t.wait_for("AAA-MARKER", timeout=1.0)

        cells = await t.chars()                   # NDArray[uint32], (rows, cols)
        has_a = bool(np.any(cells == ord("A")))
        styled = await t.styled_cells()
        bolds = [(r, c) for r, row in enumerate(styled)
                        for c, cell in enumerate(row) if cell.bold]
        reds = [cell.char for row in styled for cell in row if cell.fg == 1]

asyncio.run(main())
```

### Jupyter notebooks

A notebook cell already runs inside an event loop, so drive a terminal across
cells without `async with`: construct the handle once, `await` its methods cell
by cell, and `await t.close()` when done.

```python
from tui import Tui

t = Tui("htop", rows=30, cols=110)   # cell 1: spawn
```

```python
await t.snapshot()                   # cell 2: renders the screen in color
```

Evaluating `await t.snapshot()` as a cell's last expression shows a colored
monospace render of the viewport (`Snapshot._repr_html_`). It captures per-cell
styling by default; pass `styled=False` when you only want text. A `Snapshot`
returned by `wait_for` is text-only (the poll loop skips the styling read), so
take a fresh `await t.snapshot()` if you want color after a wait.

#### Theming

The render resolves colors through a `Theme` (default `fg`/`bg` plus the 16 ANSI
palette entries; the 16-255 cube and grayscale are not themeable). The bundled
`DARK_THEME` and `LIGHT_THEME` cover the common cases, and `DEFAULT_THEME`
(initially `DARK_THEME`) is what `_repr_html_` uses. Point it at your own
terminal by parsing a [ghostty](https://ghostty.org/) theme file:

```python
import tui
tui.DEFAULT_THEME = tui.Theme.from_ghostty("~/.config/ghostty/themes/custom-dark")
await t.snapshot()                    # now rendered in your ghostty colors
```

`from_ghostty` accepts a path or the theme text directly and reads its
`background`, `foreground`, and `palette = N=RRGGBB` lines. For a one-off render
without changing the default, call `snap.to_html(theme=...)`.

A practical gotcha when spawning through `nix run`: the cold build counts against
`wait_for`'s timeout, so the first run of an uncached binary can time out before
it ever draws. Warm the binary once, or give the first `wait_for` a generous
timeout.

### Handle timeouts

```python
import asyncio
from tui import Tui, WaitTimeout

async def main() -> None:
    async with Tui("bash", "--norc", "-i") as t:
        try:
            await t.wait_for("never gonna happen", timeout=0.25)
        except WaitTimeout as exc:
            print("missed it:", exc)

asyncio.run(main())
```

### List live instances

`Tui.list_all()` returns every `Tui` still tracked in this process. Both it and
the constructor are synchronous, so a quick census needs no event loop:

```python
from tui import Tui

Tui("bash", "--norc", "-i")
Tui("python", "-q")
print(len(Tui.list_all()))   # 2
```

### Process lifecycle

A spawned process is more than a screen: you can wait on it, read its exit
code, and stop it for real.

```python
import asyncio
from tui import Tui

async def main() -> None:
    t = Tui("bash", "-c", "echo hi; exit 7")
    code = await t.wait(timeout=3)     # blocks until exit; raises WaitTimeout on deadline
    print(code, t.is_alive())          # 7 False
    print(t.exit_code)                 # 7 (None while running or if killed by a signal)

asyncio.run(main())
```

`await t.interrupt()` sends a cooperative Ctrl+C. `await t.kill()` sends
`SIGKILL`, which a program that traps interrupts (an editor in normal mode, a
stuck REPL) cannot ignore. `await t.close()` force-kills and drops the terminal
from `list_all()` and the dashboard; `async with` blocks call it on exit, so an
editor left open still goes away.

A terminal whose child has exited keeps its final screen readable, so you can
inspect output after the fact; writing to it raises instead.

### Web dashboard

`serve()` starts a read-only web dashboard that shows a live grid of every
`Tui` in this process. The server, the [Loro][loro] CRDT document, and the
Server-Sent-Events stream all live in Rust; the browser holds its own
`loro-crdt` document and paints each terminal's viewport. Exited terminals stay
on the grid, dimmed and marked `exited`.

```python
import asyncio
from tui import Tui, serve

async def main() -> None:
    Tui("bash", "--norc", "-i")
    Tui("python", "-q")

    dash = await serve(port=8080)  # returns the handle once the server is bound
    print(dash.url)                # http://127.0.0.1:8080/
    # dash.open()                  # open in the default browser
    ...
    await dash.stop()              # or: `async with await serve() as dash:`

asyncio.run(main())
```

Pass `host="0.0.0.0"` to expose it on the network (the host must be an IP
literal), `port=0` for an ephemeral port read back from `dash.url`, and `poll`
to tune the viewport sampling interval in seconds.

[loro]: https://loro.dev/

### One dashboard across many processes

`serve()` only sees the terminals in its own process, and each call binds its
own port. When several processes run at once (for example multiple agents),
`publish()` instead exposes this process's terminals over a unix socket and a
single standalone aggregator renders all of them in one grid.

```python
import asyncio
from tui import Tui, publish

async def main() -> None:
    Tui("bash", "--norc", "-i")
    async with await publish() as pub:   # socket in the discovery dir
        print(pub.producer_id)           # this process's scope on the grid
        await asyncio.sleep(3600)

asyncio.run(main())
```

Run the aggregator once, separately, to watch every publisher:

```sh
nix run .#tui-dashboard          # http://127.0.0.1:8080/
```

Producers come and go freely: the aggregator discovers each socket in the shared
directory (`socket_dir()`) and drops a producer's terminals when it disconnects.
No process owns the server, so there is no port collision and one URL shows them
all. Pass `path=` to `publish()` to choose the socket path.

## Public surface

| Name           | Purpose                                                |
| -------------- | ------------------------------------------------------ |
| `Tui`          | One spawned process. Construct it and you have a PTY.  |
| `Snapshot`     | Frozen viewport + scrollback + size + per-cell styling. Renders to color HTML in Jupyter. |
| `Theme`        | Render colors: `fg`/`bg` + 16 ANSI. `Theme.from_ghostty(...)`. |
| `DARK_THEME` / `LIGHT_THEME` / `DEFAULT_THEME` | Bundled themes; reassign `DEFAULT_THEME` to restyle. |
| `Size`         | `(rows, cols)` dataclass.                              |
| `Key`          | ANSI keystroke constants + `Key.ctrl`/`Key.alt`.       |
| `StyledCell`   | One cell: `char`, `fg`/`bg`, and VT100 attributes.     |
| `Color`        | `None` (default), `int` (palette), or `(r, g, b)`.     |
| `WaitTimeout`  | Raised by `wait_for` / `wait` on deadline expiry.      |
| `serve`        | Await to start the web dashboard for every live `Tui`. |
| `Dashboard`    | Handle to a running dashboard: `url`, `open`, `stop`.  |
| `publish`      | Await to expose this process's terminals on a socket.  |
| `Publisher`    | Handle to a running producer: `path`, `producer_id`, `stop`. |
| `socket_dir`   | The discovery directory producers and the aggregator share. |

[pyo3-async-runtimes]: https://docs.rs/pyo3-async-runtimes/
