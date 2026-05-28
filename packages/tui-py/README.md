# superglide-tui

Python bindings for the [`tui`](../tui) Rust crate. Spawn and control multiple
PTY-backed processes from Python with full vt100 emulation, scrollback, and
optional NumPy access to per-cell character data.

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

## Use

```python
from superglide_tui import TuiManager

manager = TuiManager()
instance = manager.spawn("vim", ["-u", "NONE"])

manager.write(instance, ":help\n")
lines = manager.read_blocking(instance, timeout_ms=1_000)
for line in lines:
    print(line)

# A uint32 NumPy array of Unicode codepoints, shape (rows, cols)
codepoints = manager.read_chars_array(instance)
```

[maturin]: https://www.maturin.rs/
