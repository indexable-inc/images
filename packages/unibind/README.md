# unibind

One Rust attribute surface, one language-agnostic interface representation,
one code generator per target language. A crate annotates the functions,
records, and errors it wants to expose; unibind lowers that surface into an
IR at macro time and renders bindings through the incumbent binding library
of each enabled language backend.

## The bet

UniFFI-style tools settle for a C-ABI lowest common denominator: every value
crosses a serialization shim, and async, cancellation, and resource cleanup
are bolted on. unibind inverts that. The interface definition stays
write-once, but each backend emits code for the best binding library in its
ecosystem (pyo3 for Python, napi-rs for TypeScript, rustler for Elixir), so
every language gets native semantics: real exception hierarchies, native
async and cancellation, RAII-shaped resource cleanup, and types that flow end
to end with no RustBuffer tax.

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
- `#[unibind::object]` reserves the surface for stateful handles; it errors
  until phase 2 (#1992).
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

Crates:

- `core`: the IR types (`Interface`, functions, records, enums, errors,
  objects, the boundary `Type`), the syn lowering, and the link-section
  embed. Enums, objects, and async exist in the IR but phase 0 rejects them
  with pointers at the phase that ships them.
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

## Phases

| Phase | Issue | Scope |
| ----- | ----- | ----- |
| 0     | #1990 | core IR, macro skeleton, pyo3 backend for sync functions, records, errors; proven by porting `packages/code/scipql/py` |
| 1     | #1991 | `unibind-gen`: host files (`.pyi`) from the embedded IR, `unibind.lib.build` nix glue |
| 2     | #1992 | async, cancellation, streams, resources/objects (Python backend) |
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
