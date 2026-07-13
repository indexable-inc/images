---
paths: "**/*.rs, **/Cargo.toml, **/Cargo.lock"
---

# Rust Coding Conventions

## Project Initialization

When creating a new Rust project:
- Use workspace-first architecture with crates in `crates/`
- Use hierarchical directory structure to group related crates
- Edition 2024 with resolver 3
- Always create `rust-toolchain.toml` with latest stable version

## Core Dependencies

### Error Handling: `eyre` + `color_eyre`

```toml
[dependencies]
eyre = "0.6"
color_eyre = "0.6"
```

**Use `eyre`, NEVER `anyhow`.** This is non-negotiable.

### Logging: `tracing`

```toml
[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

**Use `tracing` macros, NOT `println!` or `eprintln!` in async code.**

### Serialization: `rkyv` over `bincode`/`serde_json`

For binary serialization where performance matters, prefer `rkyv` (zero-copy deserialization):

```toml
[dependencies]
rkyv = { version = "0.8", features = ["validation"] }
```

**Use `rkyv` for:**
- Cache storage (LMDB, file caches)
- High-frequency serialization/deserialization
- Memory-mapped data structures

**Use `serde_json` for:**
- Config files (human-readable)
- API responses
- Debug output

```rust
// rkyv provides zero-copy access - data is read directly from bytes
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct CachedData {
    chunks: Vec<Chunk>,
}

// Serialize
let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&data)?;

// Zero-copy deserialize (no allocation!)
let archived = rkyv::access::<ArchivedCachedData, rkyv::rancor::Error>(&bytes)?;
```

## Import Style (CRITICAL)

**ALWAYS use full inline paths. NEVER import at the top of the file.**

```rust
// GOOD - full inline paths, always
fn process() -> eyre::Result<()> {
    let map = std::collections::HashMap::new();
    let path = std::path::PathBuf::from("foo");
    tracing::info!("processing");
    eyre::bail!("something went wrong");
}

// BAD - imports at top of file
use std::collections::HashMap;
use tracing::info;
use eyre::{bail, eyre, Result};  // NO! Use eyre::Result, eyre::bail!, etc.
```

**This applies to EVERYTHING:**
- `eyre::Result`, `eyre::bail!`, `eyre::eyre!`, `eyre::WrapErr`
- `std::collections::HashMap`, `std::path::PathBuf`
- `tracing::info!`, `tracing::debug!`, `tracing::error!`
- All macros, types, traits, and functions

**Only exception:** Trait methods need `use Trait as _;` for method resolution (see below).

### Trait Methods: Use `as _` Imports

```rust
// GOOD - import trait as _, call method directly
use std::io::Write as _;
writer.write_all(&data)?;

// BAD - verbose fully qualified syntax
std::io::Write::write_all(&mut writer, &data)?;
```

### Intermediate Variables Over Nested Calls

```rust
// GOOD - intermediate variables, shadowing is fine
let file = std::fs::File::create(path)?;
let mut writer = std::io::BufWriter::new(file);

// BAD - nested, hard to read
let mut writer = std::io::BufWriter::new(std::fs::File::create(path)?);
```

## Error Handling

### Always Add Context with `.wrap_err()`

**Almost every `?` needs `.wrap_err()` or `.wrap_err_with()`.** Bare `?` loses the "why":

```rust
// BAD - user sees "No such file or directory" with no clue what file
let data = std::fs::read_to_string(&path)?;

// GOOD - full context chain shows exactly what happened
let data = std::fs::read_to_string(&path)
    .wrap_err_with(|| format!("failed to read config from {path:?}"))?;
```

### Write Human-Friendly Errors

```rust
// BAD - cryptic, unhelpful
eyre::bail!("invalid config");

// GOOD - explains the problem clearly
eyre::bail!("config file not found at {path:?} — create one with `myapp init`");
```

## Module Structure

**`mod.rs` is deprecated syntax.** Use modern file naming:

```
src/
├── lib.rs          # declares: mod parser; mod utils;
├── parser.rs       # the parser module
└── utils.rs        # the utils module
```

## Workspace Configuration

```toml
# Cargo.toml (workspace root)
[workspace]
resolver = "3"
members = ["crates/my/*", "crates/my-cli"]

[workspace.package]
edition = "2024"

[workspace.dependencies]
eyre = "0.6"
my-core = { path = "crates/my/core" }
```

Individual crates use `workspace = true`:

```toml
[dependencies]
eyre.workspace = true
my-core.workspace = true
```

## Testing

Use `cargo nextest run` instead of `cargo test`.

## GitHub Actions

**Use `actions-rust-lang/setup-rust-toolchain@v1`** for CI. It auto-detects `rust-toolchain.toml`:

```yaml
steps:
  - uses: actions/checkout@v4
  - uses: actions-rust-lang/setup-rust-toolchain@v1
  - run: cargo build --release
```

For cross-compilation, specify targets:

```yaml
- uses: actions-rust-lang/setup-rust-toolchain@v1
  with:
    target: aarch64-apple-darwin
```

## Checking Code: `cargo check` over `cargo clippy`

**Use `cargo check` for fast iteration.** It's significantly faster than clippy and gives you type errors immediately:

```bash
# Fast type checking
cargo check

# For a specific package
cargo check -p my-crate
```

**Why `cargo check` over `cargo clippy`:**
- 2-10x faster compilation (no lint analysis)
- Sufficient for catching type errors during development
- Run clippy only in CI or before committing

**When to run clippy:**
- In CI pipelines (automated)
- Before committing (final polish)
- When specifically asked to lint

If you do need to fix clippy warnings, use `cargo clippy --fix`:

```bash
cargo clippy --fix --allow-dirty --allow-staged
```

## Prefer Modules Over Unit Structs for Namespacing

**Use `mod` for grouping constants, not unit structs with `impl` blocks.**

```rust
// BAD - unit struct used only for namespacing
struct Status;

impl Status {
    const CONNECTING: &str = "Connecting";
    const CONNECTED: &str = "Connected";
}
// Usage: Status::CONNECTING

// GOOD - module provides cleaner namespacing
mod status {
    pub const CONNECTING: &str = "Connecting";
    pub const CONNECTED: &str = "Connected";
}
// Usage: status::CONNECTING
```

Modules are the idiomatic Rust way to namespace related items. Unit structs should be used when you need to implement traits or create instances.

## No Magic Numbers

```rust
// BAD
password.len() >= 8

// GOOD
const MIN_PASSWORD_LENGTH: usize = 8;
password.len() >= MIN_PASSWORD_LENGTH
```

## Handling Impossible States

```rust
// GOOD - explicit match documents why this is safe
let value = match map.get(&key) {
    Some(v) => v,
    None => unreachable!("key {key:?} was inserted in initialization"),
};

// BAD - unwrap hides the invariant
let value = map.get(&key).unwrap();
```

## Safe Indexing in Binary Parsing

For low-level binary protocol parsing, use `.get()` with `unreachable!` instead of direct indexing:

```rust
// BAD - panics on invalid data, clippy warns
let byte = data[pos];
let slice = &data[pos..pos + 4];

// GOOD - explicit bounds check with documented invariant
let byte = match data.get(pos) {
    Some(&b) => b,
    None => unreachable!("pos {pos} validated against data.len() {}", data.len()),
};

let slice = data.get(pos..pos + 4)
    .ok_or_else(|| eyre::eyre!("unexpected EOF at pos {pos}"))?;
```

For truly known-safe cases (e.g., after length validation), `unreachable!` documents the invariant.
For external data, prefer returning errors over panicking.

## Defensive Programming (CRITICAL)

**Never silently swallow errors.** Every failure path must be explicit and traceable:

```rust
// BAD - silently breaks, impossible to debug
let Some(data) = buffer.get(pos..end) else {
    break;  // Silent failure - where did parsing stop? Why?
};

// BAD - continues without handling the error
let Some(value) = parse_value(input) else {
    continue;  // Skipping silently
};

// GOOD - explicit error with full context
let data = buffer.get(pos..end)
    .ok_or_else(|| color_eyre::eyre::eyre!("buffer underflow: need bytes {}..{}, have {}", pos, end, buffer.len()))?;

// GOOD - bail! for early return
let Some(data) = buffer.get(pos..end) else {
    color_eyre::eyre::bail!("buffer underflow: need bytes {}..{}, have {}", pos, end, buffer.len());
};

// GOOD - if truly unreachable after validation, document the invariant
let data = buffer.get(pos..end)
    .expect("bounds validated: header declared size fits in buffer");
```

**Error handling hierarchy (in order of preference):**
1. `?` with `.wrap_err()` / `.wrap_err_with()` - propagate with context
2. `eyre::bail!("message")` - cleaner than `return Err(eyre::eyre!(...))`
3. `.expect("invariant reason")` - for impossible states after validation
4. `unreachable!("reason")` - for match arms that logically cannot occur
5. **FORBIDDEN**: silent `break`, empty `continue`, `.ok()`, `.unwrap_or_default()` for errors

**CRITICAL: Always use full paths.** Never import macros or functions at the top:
```rust
// GOOD - full inline paths, always
color_eyre::eyre::bail!("failed to parse");
color_eyre::eyre::eyre!("error message");
tracing::info!("log message");

// BAD - imports at top, then bare names
use color_eyre::eyre::{bail, eyre};
bail!("failed");  // NO!
```

Note: The crate is `color_eyre`, not bare `eyre`. Use `color_eyre::eyre::bail!`, not `eyre::bail!`.

**The goal:** When something fails, the error message should contain enough context to debug without a debugger.

## Nesting Depth

Maximum 4 levels. Use early returns and `let-else`:

```rust
fn process(input: Option<Data>) -> eyre::Result<()> {
    let Some(data) = input else {
        return Ok(());
    };
    // ...
}
```

## Exit Codes: Use `ExitCode`, Not `process::exit`

**Never use `std::process::exit()`.** It bypasses destructors and is disallowed by clippy.

```rust
// BAD - bypasses destructors, clippy error
fn main() {
    if something_failed {
        std::process::exit(1);
    }
}

// GOOD - return ExitCode from main
fn main() -> std::process::ExitCode {
    if something_failed {
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

// GOOD - return Result, let main handle it
fn main() -> eyre::Result<()> {
    do_stuff()?;  // Errors propagate up
    Ok(())
}

// GOOD - for CLI apps, return Result and use color_eyre
fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    run()
}
```

**When to use `ExitCode`:**
- Binary needs specific exit codes (0, 1, 2, etc.)
- Integrating with shell scripts that check `$?`

**When to use `Result`:**
- Most CLI applications (let eyre format the error)
- Libraries (always return `Result`)

## Function Signatures: Prefer `impl Trait`

**Prefer `&impl Trait` / `&mut impl Trait` over complex concrete types** in function parameters when you don't need the specific type:

```rust
// BAD - verbose concrete type
fn skip_until_alpha(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    // ...
}

// GOOD - simpler, more generic
fn skip_until_alpha(chars: &mut impl Iterator<Item = char>) {
    // ...
}

// BAD - concrete type when trait suffices
fn process_reader(reader: &mut std::io::BufReader<std::fs::File>) {
    // ...
}

// GOOD - accepts any reader
fn process_reader(reader: &mut impl std::io::BufRead) {
    // ...
}
```

This improves readability and allows more flexible usage without sacrificing performance (monomorphization still happens).

## Return `impl Iterator` Over `Vec` When Possible

**Prefer returning `impl Iterator` instead of `Vec` when the caller will iterate or chain operations:**

```rust
// BAD - allocates Vec even if caller only needs first 3 items
fn get_matching_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension() == Some("rs".as_ref()))
        .collect()  // Forces allocation
}

// GOOD - lazy, no allocation, caller decides
fn get_matching_files(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension() == Some("rs".as_ref()))
}

// Caller can now:
// - Take first N: .take(3).collect()
// - Find one: .find(|p| p.file_name() == "lib.rs")
// - Chain more: .filter(...).map(...)
// - Collect if needed: .collect::<Vec<_>>()
```

**When to return `Vec` instead:**
- Caller needs random access or `.len()`
- Data must outlive the iterator's borrows
- Iterator would be consumed multiple times
- API stability (changing return type is breaking)

**When to return `impl Iterator`:**
- Caller will iterate once (for loop, `.for_each()`, `.collect()`)
- Enables lazy evaluation / short-circuiting
- Allows chaining without intermediate allocations
- Internal implementation detail (not public API)

## Named Structs Over Anonymous Tuples

**NEVER return anonymous tuples with more than 2 elements.** Use named structs instead:

```rust
// BAD - what do these fields mean?
fn execute_hook(hook: Hook) -> (String, HookResult, bool) {
    // ...
    (hook_id, result, was_skipped)  // Cryptic!
}

// GOOD - self-documenting
struct HookExecution {
    hook_id: String,
    result: HookResult,
    skipped: bool,
}

fn execute_hook(hook: Hook) -> HookExecution {
    // ...
    HookExecution { hook_id, result, skipped }
}
```

**Rules:**
- 2-tuples are OK for simple pairs: `(key, value)`, `(index, item)`
- 3+ element tuples: always use a named struct
- Return types especially benefit from named fields (callers see field names)

## Config Structs: Use `#[non_exhaustive]`

**Mark config/options structs with `#[non_exhaustive]`** to allow adding fields without breaking downstream code:

```rust
// GOOD - can add fields in future minor versions
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ExportConfig {
    pub format: Format,
    pub pretty: bool,
    pub include_metadata: bool,
}

impl ExportConfig {
    /// Builder method for format
    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Builder method for pretty printing
    pub fn pretty(mut self, pretty: bool) -> Self {
        self.pretty = pretty;
        self
    }
}

// Usage - fluent builder pattern
let config = ExportConfig::default()
    .format(Format::Json)
    .pretty(true);
```

**Why `#[non_exhaustive]`:**
- Prevents `ExportConfig { format: .., pretty: .., include_metadata: .. }` struct literal syntax outside crate
- Forces users to use `Default::default()` + builder methods
- Adding new fields is non-breaking (users can't exhaustively match)

**Always pair with:**
1. `#[derive(Default)]` - so users can create instances
2. Builder methods - for setting each field fluently
3. Public fields - still readable, just can't construct directly

```rust
// BAD - adding fields breaks downstream code
pub struct Config {
    pub timeout: Duration,
}
// Later adding `pub retries: u32` breaks: `Config { timeout: .. }`

// GOOD - non_exhaustive allows evolution
#[non_exhaustive]
pub struct Config {
    pub timeout: Duration,
}
// Adding `pub retries: u32` is safe - users use Config::default().timeout(..)
```

## File Walking with `ignore` Crate

**When using `ignore::WalkBuilder` with `.hidden(false)`, you MUST manually filter `.git`.**

The `ignore` crate skips `.git` by default because it's a hidden directory. If you disable hidden filtering to include files like `.env.example`, `.git/` will be included too.

```rust
// BAD - includes .git directory contents!
let mut builder = ignore::WalkBuilder::new(&root);
builder
    .hidden(false)  // Include hidden files like .env.example
    .git_ignore(true);

// GOOD - explicitly filter .git
let mut builder = ignore::WalkBuilder::new(&root);
builder
    .hidden(false)
    .git_ignore(true)
    .git_global(true)
    .git_exclude(true)
    .require_git(false)  // Work even without .git directory
    .filter_entry(|e| e.file_name() != ".git");
```

**Key points:**
- `.hidden(true)` (default) skips ALL hidden files including `.git`
- `.hidden(false)` includes ALL hidden files including `.git`
- `.git_ignore(true)` respects `.gitignore` but does NOT skip `.git` directory itself
- Always use `.filter_entry()` to skip `.git` when `hidden(false)`

## File Locking for Concurrent Access

**When writing files that may be accessed concurrently, use file locking to prevent corruption.**

As of Rust 1.89, `File::lock` is in std:

```rust
use std::io::Write as _;

let mut file = std::fs::OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .open("sessions.json")?;
file.lock()?;  // Exclusive lock - blocks until acquired
file.write_all(data.as_bytes())?;
// Lock released automatically on drop
```

**Lock types:**
- `file.lock()` - exclusive lock for writing (only one writer)
- `file.lock_shared()` - shared lock for reading (multiple readers OK)
- `file.try_lock()` / `file.try_lock_shared()` - non-blocking variants

**Key points:**
- Lock the actual file you're writing - no separate `.lock` file needed
- Locks are advisory on Unix (processes must cooperate)
- Locks auto-release on drop
- Use for: config files, session state, caches with concurrent writers

## PyO3: Always Release the GIL for CPU-Bound Work

**When writing pyo3 bindings, ALWAYS release the GIL during CPU-bound Rust computation.**

Without releasing the GIL, Python's `ThreadPoolExecutor` cannot achieve true parallelism - only one thread runs at a time despite multiple threads being spawned.

```rust
// BAD - holds GIL throughout, no parallelism from Python threads
#[pyfunction]
fn process(py: Python<'_>, data: &[u8]) -> PyResult<Vec<u8>> {
    let result = expensive_computation(data);  // GIL held!
    Ok(result)
}

// GOOD - releases GIL, enables true parallelism
#[pyfunction]
fn process(py: Python<'_>, data: &[u8]) -> PyResult<Vec<u8>> {
    // py.detach() releases GIL during the closure
    let result = py.detach(|| expensive_computation(data));
    Ok(result)
}
```

**Why this matters:**
- Python's GIL prevents multiple threads from executing Python bytecode simultaneously
- pyo3 functions hold the GIL by default, even for pure Rust code
- `py.detach()` releases the GIL, allowing other Python threads to run AND enabling parallel Rust execution
- Without this, `ThreadPoolExecutor` in Python will serialize all calls

**When to use `py.detach()`:**
- Any CPU-bound computation (image processing, parsing, compression)
- I/O operations that don't need Python objects
- Anything that takes >1ms and doesn't touch Python objects

**When NOT to use:**
- When accessing Python objects inside the closure (they require GIL)
- Very short operations where GIL overhead dominates

## Unsafe Code: `#[expect]` Reasons Must Explain WHY It's Safe

When using `#[expect(unsafe_code, reason = "...")]`, the reason must explain **WHY the code is safe**, not just that it needs to be unsafe.

```rust
// BAD - just says it's unsafe, doesn't explain safety
#[expect(unsafe_code, reason = "must be unsafe - applies sandbox to process")]
pub unsafe fn apply_sandbox(profile: &str) -> Result<(), String> { ... }

// BAD - restates the obvious
#[expect(unsafe_code, reason = "FFI requires unsafe")]
unsafe extern "C" { ... }

// GOOD - explains the safety invariants
#[expect(
    unsafe_code,
    reason = "FFI to stable macOS sandbox APIs with proper lifetime and null handling"
)]

// GOOD - module-level with safety documentation
//! # Safety
//!
//! This module uses FFI to call macOS system functions. Safety is ensured by:
//! - Using stable C ABI functions from libsystem_sandbox.dylib
//! - Passing valid CString pointers that outlive the FFI call
//! - Properly freeing error buffers allocated by sandbox_init
//! - Only reading from error pointer after null check
#![expect(
    unsafe_code,
    reason = "FFI to stable macOS sandbox APIs with proper lifetime and null handling"
)]
```

**The reason should answer:**
1. Why is this operation safe despite being unsafe?
2. What invariants are maintained?
3. What guarantees do we rely on? (stable ABI, pointer validity, lifetime bounds)

**For FFI specifically:**
- Mention the API stability (system library, documented ABI)
- Note pointer validity and lifetime guarantees
- Explain null checks and error handling
- Reference memory ownership (who allocates, who frees)

**For internal unsafe blocks, use `// SAFETY:` comments:**
```rust
// SAFETY: error is non-null and points to a valid C string allocated by sandbox_init
let msg = unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned();

// SAFETY: error was allocated by sandbox_init, freed with matching sandbox_free_error
unsafe { sandbox_free_error(error) };
```
