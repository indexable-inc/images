<p align="center"><img src="assets/hero.svg" width="720" alt="an annotated Rust module is lowered by syn into an Interface IR, which renders through pyo3 today and napi-rs and rustler in planned phases"></p>

# unibind

Why should every language binding pay a C-ABI serialization tax when pyo3, napi-rs, and rustler already exist? unibind is one Rust attribute surface, one language-agnostic interface representation, and one code generator per target language: a crate annotates the functions, records, and errors it wants to expose, unibind lowers that surface into an IR at macro time, and each backend renders bindings through the incumbent binding library of its ecosystem.

## The bet

UniFFI-style tools settle for a C-ABI lowest common denominator: every value
crosses a serialization shim, and async, cancellation, and resource cleanup
are bolted on. unibind inverts that. The interface definition stays
write-once, but each backend emits code for the best binding library in its
ecosystem (pyo3 for Python, napi-rs for TypeScript, rustler for Elixir), so
every language gets native semantics: real exception hierarchies, native
async and cancellation, RAII-shaped resource cleanup, and types that flow end
to end with no RustBuffer tax.

## Use it

unibind is a proc-macro library consumed inside this workspace. Depend on it
with the backend you want as a feature, plus the binding library the backend
targets:

```toml
[dependencies]
unibind = { workspace = true, features = ["py"] }
pyo3 = { workspace = true, features = ["extension-module"] }
```

(`packages/code/scipql/py` is the reference consumer.) The workspace lives in
the monorepo: `git clone https://github.com/indexable-inc/index`.

## Surface

There is no UDL or spec file. The Rust module is the source of truth:

```rust
#[unibind::export]
mod _mylib {
    /// Rows come back as native classes.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Row {
        pub id: u64,
        pub name: String,
    }

    /// Everything the boundary raises.
    #[unibind::error(py(base = "ValueError"))]
    pub enum MyError {
        /// The store is gone.
        StoreGone { message: String },
    }

    /// Doc comments become docstrings.
    pub fn rows(store: &str, #[unibind(default = 10)] limit: usize) -> Result<Vec<Row>, MyError> {
        ...
    }
}
```

- `#[unibind::export]` on an inline module lowers every `pub fn` in it, plus
  the annotated types, into one interface value in a single parse. Private
  items pass through as plain Rust.
- `#[unibind::record]` marks a plain-data struct that crosses the boundary by
  value: a native class per language, one read-only attribute per field, and
  a positional constructor. Fields are `pub` and owned; the struct derives
  `Clone`.
- `#[unibind::error]` marks an error enum. Each variant becomes an exception
  class under one base class named after the enum; `py(base = "...")` picks
  the built-in the base extends. The enum implements `Display`, and the
  raised exception carries that text.
- `#[unibind::object]` marks a stateful handle: inherent methods (implicit
  `&self`) become methods on a native class, and `#[unibind(constructor)]`
  names the receiver-less constructor. `object(resource)` adds
  deterministic cleanup: a generated `close()`, `async with` support, and
  a `ResourceWarning` when the handle is dropped unclosed.
- `#[unibind(py(name = "..."))]` renames a module, function, argument, field,
  or error variant for Python. `#[unibind(default = ...)]` gives an argument
  a default; `Option` arguments default to `None` automatically.

## Pipeline

```
annotated module --syn lowering--> Interface IR --backend render--> binding code
     (macros)      (core)                           (backend-py, ...)
```

The `unibind` proc-macro crate parses the module once, `unibind-core` lowers
it to the IR and validates the surface, and each backend enabled by a cargo
feature renders code into the expansion. The serialized IR also lands in a
link section of the built artifact (`.unibind_ir`, `__DATA,__unibind_ir` on
Apple), wasm-bindgen style, so out-of-process generators in later phases can
read the interface without the Rust source: generated `.pyi` stubs and nix
glue are phase 1 (#1991), `.d.ts` and Elixir specs come with their backends.

Phase 1 ships that out-of-process half. `unibind-gen` (the `gen` crate) reads
the section back and renders host files through a small
`HostFile`/`HostEmitter` seam -- Python today (`<module>.pyi`, `py.typed`,
and a wrapper `__init__.py` unless the package hands one in); the `.d.ts`
(#1993) and `.ex` (#1995) emitters implement the same seam. On the nix side,
`unibind.lib.build { crate; targets.py = { package; ... }; }`
(`ix.unibind` / `index.lib.unibind`, packages/unibind/nix) glues it in: the
cdylib comes from the shared workspace graph, the registry's
`pyExtension = true` marker injects the darwin `-undefined dynamic_lookup`
link args (replacing per-crate build.rs), and the outputs are the merged
python site tree, a zuban/ruff strict gate, the mcp-style importable module,
and the Linux wheel. `packages/code/scipql/py` is the proving consumer: its
hand-written `_scipql.pyi` and `py.typed` are deleted and generated instead.

Crates:

- `core`: the IR types (`Interface`, functions, records, enums, errors,
  objects, the boundary `Type`), the syn lowering, and the link-section
  embed. Phase 2 turned on async, streams, and
  objects; plain (non-error) enums still wait for their phase.
- `gen`: the `unibind-gen` binary. Reads the embedded IR out of a compiled
  artifact and emits the host-language files above; run at build time by
  `unibind.lib.build`, never at macro time.
- `macros`: the `unibind` proc-macro crate. Parse once to IR, dispatch to the
  backends the consuming crate enabled through features (`py` today).
- `backend-py`: renders the IR into pyo3 0.28 (abi3-py311) code:
  `#[pyfunction]` wrappers with `#[pyo3(signature = ...)]` defaults,
  `#[pyclass]` records, `create_exception!` hierarchies plus a
  `From<YourError> for PyErr` impl, and one imperative `#[pymodule]` that
  registers everything and sets `__version__`. Doc comments become
  docstrings. The consuming crate depends on `pyo3` directly with
  `extension-module`.

## Type mapping (phase 0)

| Rust                  | IR              | Python        |
| --------------------- | --------------- | ------------- |
| `bool`                | `Bool`          | `bool`        |
| `i8..i64`, `u8..u64`, `isize`, `usize` | `Int` | `int` |
| `f32`, `f64`          | `Float`         | `float`       |
| `String` / `&str`     | `String`        | `str`         |
| `PathBuf` / `&Path`   | `Path`          | accepts `str \| os.PathLike`, returns `str` |
| `Vec<u8>` / `&[u8]`   | `Bytes`         | `bytes`       |
| `Option<T>`           | `Option`        | `T \| None`   |
| `Vec<T>`              | `Vec`           | `list[T]`     |
| `HashMap<K, V>`       | `Map`           | `dict[K, V]`  |
| `#[unibind::record]`  | `Named`         | native class  |
| `Result<T, E>`        | `ret` + `throws`| `T`, raises `E`'s hierarchy |

Borrowed forms (`&str`, `&Path`, `&[u8]`, including under `Option`) are
argument-only; returns and record fields own their data.

Phase 1 changes nothing in this table: the `.pyi` emitter renders these same
rules (argument vs return position included) from the untouched IR.

## Phase 2 surface

- `pub async fn` exports as a real asyncio coroutine on the shared tokio
  runtime. Cancellation is true cancellation: cancelling the Python task
  drops the in-flight Rust future, so guards, locks, and connections
  release immediately instead of leaking on a detached task.
- `fn ... -> UniStream<T>` (bare or behind `async fn`/`Result`) exports as
  an async iterator. It is pull-based: each `__anext__` polls exactly one
  item, so a consumer that stops early stops the producer with it.
- `#[unibind(blocking)]` runs the call with the GIL released, for
  CPU-bound or thread-sleeping work that must not stall the interpreter.
- `&[u8]` arguments cross through the buffer protocol with no copy:
  `bytes`, `bytearray`, and contiguous `memoryview` all alias the caller's
  memory for the duration of the call.
- A crate exporting async functions, streams, or objects adds
  `unibind-runtime` with the `py` feature next to its `unibind` dependency;
  sync-only crates (scipql-py) do not need it.

## Conformance suite

`packages/unibind/conformance` is the runtime proof for everything above:
a cdylib exporting the full phase-2 surface plus a stdlib-only `runner.py`
that asserts the semantics from Python with quantitative evidence, such as
live/dropped guard counts around `task.cancel()`, produced-vs-consumed
stream counters, exactly one `ResourceWarning` per leaked resource, and
`ctypes.addressof` equality for zero-copy buffers. It runs in CI as
`checks.<system>.unibind-conformance-run`.

## Phases

| Phase | Issue | Scope |
| ----- | ----- | ----- |
| 0     | #1990 | core IR, macro skeleton, pyo3 backend for sync functions, records, errors; proven by porting `packages/code/scipql/py` |
| 1     | #1991 | `unibind-gen`: host files (`.pyi`) from the embedded IR, `unibind.lib.build` nix glue |
| 2     | #1992 | async, cancellation, streams, resources/objects (Python backend); proven by `packages/unibind/conformance` |
| 3     | #1993 | TypeScript backend (napi-rs) with enriched `.d.ts` |
| 4     | #1994 | Rust client backend over a stable ABI |
| 5     | #1995 | Elixir backend (rustler, generated `.ex`, `@spec`) |
| 6     | #1996 | adopt for ix-sdk, delete sdk-py and sdk-ts |

## Phase 0 in the tree

`packages/code/scipql/py` is the proving port: the same five functions, the
same `_scipql` module name and cdylib layout the mcp interpreter bundles, but
the 169 lines of hand-written pyo3 conversion replaced by the annotated
module above plus record and error declarations. The exception surface
stays compatible (`ScipqlError` extends `ValueError`, which is what the
hand-written binding raised), and `packages/unibind/backend-py/tests`
snapshots the exact code the macro generates.
