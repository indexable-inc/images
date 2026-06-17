//! fff-backed `@` file completer for Claude Code, one compiled binary with two
//! subcommands:
//!
//!   * `fff-suggest query`: the per-keystroke client wired into Claude Code's
//!     `fileSuggestion` setting. Claude spawns it fresh on every keystroke after
//!     `@`, hands it `{ "query": "<text>" , … }` on stdin (cwd = the project
//!     dir), and treats each non-empty stdout line as a suggestion, used in the
//!     order returned. It round-trips the query to the resident daemon over a
//!     unix socket and prints the ranked paths. It fails OPEN and FAST: any
//!     error exits 0 with no output (Claude then shows no suggestions rather
//!     than hanging on its 5s budget).
//!
//!   * `fff-suggest serve <root>`: the resident daemon. It `dlopen`s the same
//!     `libfff_c` the notebook kernel uses (`IX_FFF_LIB`, baked by the
//!     claude-code wrapper), holds one frecency-ranked, file-watched index over
//!     `<root>`, and answers queries over the socket until it sits idle. The
//!     client auto-starts it detached on the first `@` in a project; every
//!     keystroke after that is a warm socket round-trip with no Python and no
//!     re-index.
//!
//! The socket lives at `<runtime-dir>/ix-fff-suggest/<hash(root)>.sock`; the
//! hash keeps the name short enough for the platform's `sun_path` limit. Client
//! and daemon are the same executable, so they hash an identical `root` to the
//! same path.

use std::ffi::{CStr, CString, c_char, c_void};
use std::hash::{Hash as _, Hasher as _};
use std::io::{Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use libloading::{Library, Symbol};

/// Mirrors `FFF_CREATE_OPTIONS_VERSION` in fff-c. The options struct only ever
/// appends fields, so v1 stays valid forever.
const OPTIONS_VERSION: u32 = 1;
/// Suggestions to return; Claude caps its own menu well below this.
const SEARCH_LIMIT: u32 = 40;
/// Daemon exits after this long with no connections; the next `@` respawns it.
/// Overridable via `IX_FFF_SUGGEST_IDLE_MS` (the e2e test uses a small value so
/// it never leaves a long-lived daemon behind).
const IDLE_MS: i32 = 10 * 60 * 1000;
/// Bounded wait for the initial scan before the daemon starts serving, so the
/// first keystroke gets a reasonably full index rather than an empty one.
const INITIAL_SCAN_MS: u64 = 1000;
/// Total client budget, kept under Claude's 5s `fileSuggestion` timeout.
const CLIENT_BUDGET_MS: u64 = 3500;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        // `query` is the default so the wired command can be just the binary.
        None | Some("query") => client(),
        Some("serve") => daemon(std::env::args().nth(2)),
        other => {
            eprintln!("fff-suggest: unknown subcommand {other:?}");
            ExitCode::from(2)
        }
    }
}

// ── shared ───────────────────────────────────────────────────────────────────

/// The per-user runtime directory that holds the daemon sockets. Prefer
/// `$XDG_RUNTIME_DIR`, which the OS already guarantees is a 0700 user-private
/// tmpfs. When it is absent we fall back to the world-writable system temp dir,
/// so the directory name is namespaced by uid and the directory itself is
/// created/validated 0700 (see `ensure_private_dir`); otherwise a deterministic
/// socket path under a shared `/tmp` would let another local user pre-bind or
/// connect to it to spoof completions or stall every `@`.
fn runtime_dir() -> PathBuf {
    dirs::runtime_dir().map_or_else(
        // SAFETY: `getuid` is always safe; it just reads the process's real uid.
        || std::env::temp_dir().join(format!("ix-fff-suggest-{}", unsafe { libc::getuid() })),
        |dir| dir.join("ix-fff-suggest"),
    )
}

/// The unix socket for `root`'s daemon, under the per-user `runtime_dir`. A hash
/// of the (canonical) path keeps the filename short, since `sun_path` is ~104
/// bytes on macOS. Client and daemon run the same build, so `DefaultHasher`
/// agrees between them.
fn socket_path(root: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    runtime_dir().join(format!("{:016x}.sock", hasher.finish()))
}

/// Create `dir` as a user-private 0700 directory, or accept it only if it
/// already is a real directory owned by us. Returns `false` (caller must fail
/// open, leaving no socket server) when the path exists but is not a directory,
/// is a symlink, or is owned by another user: the markers of a hijack attempt
/// under a shared `/tmp`.
///
/// An existing dir we own but with group/other bits is *repaired* to 0700 rather
/// than rejected: earlier builds created `<runtime>/ix-fff-suggest` with plain
/// `create_dir_all`, which leaves it 0755 under the usual 022 umask, so rejecting
/// it would silently disable completions across an in-session upgrade. Since the
/// dir is already ours (and, under `$XDG_RUNTIME_DIR`, inside a 0700 parent),
/// tightening it back to 0700 restores the invariant without that regression.
fn ensure_private_dir(dir: &Path) -> bool {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    // create_new-style: succeeds only if we make it, so the 0700 mode is ours.
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => return true,
        // Already there; fall through to validate it is safely ours.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return false,
    }
    // `symlink_metadata` does not follow a symlink, so a planted symlink to a
    // dir we happen to own cannot pass this check.
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    // SAFETY: `getuid` just reads the process's real uid.
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return false;
    }
    // No group/other permission bits set: the low 6 bits of the mode are zero,
    // i.e. at least 6 trailing zero bits. Repair a loose-but-owned dir in place.
    if meta.permissions().mode().trailing_zeros() >= 6 {
        return true;
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_ok()
}

// ── client ───────────────────────────────────────────────────────────────────

fn client() -> ExitCode {
    let query = read_query();
    let Ok(root) = std::env::current_dir().and_then(|c| c.canonicalize()) else {
        return ExitCode::SUCCESS;
    };
    let sock = socket_path(&root);

    // Refuse to talk to a socket under a directory that is not a user-private
    // 0700 dir we own: under a shared `/tmp` fallback another local user could
    // have planted one to spoof completions. Fail open (no suggestions).
    let Some(dir) = sock.parent() else {
        return ExitCode::SUCCESS;
    };
    if !ensure_private_dir(dir) {
        return ExitCode::SUCCESS;
    }

    // Fast path: a warm daemon is already listening.
    if let Some(out) = try_query(&sock, &query) {
        print!("{out}");
        return ExitCode::SUCCESS;
    }

    // Cold path: start the daemon detached, then poll until it binds (or budget
    // runs out). Failing here is silent: Claude just shows no suggestions.
    spawn_daemon(&root);
    let deadline = Instant::now() + Duration::from_millis(CLIENT_BUDGET_MS);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        if let Some(out) = try_query(&sock, &query) {
            print!("{out}");
            break;
        }
    }
    ExitCode::SUCCESS
}

/// The text typed after `@`, read from Claude's JSON stdin. Any missing input or
/// parse error yields an empty query (the daemon then returns top files).
fn read_query() -> String {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return String::new();
    }
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("query").and_then(|q| q.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// Connect, send the query, return the daemon's reply (possibly empty). `None`
/// means no daemon answered, which is the signal to (try to) start one.
fn try_query(sock: &Path, query: &str) -> Option<String> {
    let mut stream = UnixStream::connect(sock).ok()?;
    let budget = Duration::from_millis(CLIENT_BUDGET_MS);
    stream.set_read_timeout(Some(budget)).ok()?;
    stream.set_write_timeout(Some(budget)).ok()?;
    // One request line; newlines in a path fragment are meaningless, so drop them.
    let line = format!("{}\n", query.replace('\n', ""));
    stream.write_all(line.as_bytes()).ok()?;
    let mut out = String::new();
    stream.read_to_string(&mut out).ok()?;
    Some(out)
}

/// Re-exec ourselves as the daemon, fully detached, so it outlives this client.
fn spawn_daemon(root: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .arg("serve")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

// ── daemon ───────────────────────────────────────────────────────────────────

fn daemon(root: Option<String>) -> ExitCode {
    let Some(root) = root else {
        eprintln!("fff-suggest serve: missing <root>");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(root);
    let sock = socket_path(&root);

    let Some(listener) = bind(&sock) else {
        // A live daemon already owns this socket, or the runtime dir is not a
        // private 0700 dir we own. Either way, nothing to do.
        return ExitCode::SUCCESS;
    };

    let index = match Index::open(&root) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("fff-suggest serve: {err}");
            let _ = std::fs::remove_file(&sock);
            return ExitCode::FAILURE;
        }
    };
    // Give the initial scan a moment so the first keystroke isn't empty; clients
    // that connect meanwhile wait in the listen backlog.
    index.wait_for_scan(INITIAL_SCAN_MS);

    serve(&listener, &index);
    let _ = std::fs::remove_file(&sock);
    ExitCode::SUCCESS
}

/// Bind the socket, taking over a stale file left by a dead daemon. Returns
/// `None` when a *live* daemon already holds it (we lose the start race), or
/// when the runtime directory is not a user-private 0700 dir we own (we then
/// fail open rather than serve over a hijackable socket).
fn bind(sock: &Path) -> Option<UnixListener> {
    let parent = sock.parent()?;
    if !ensure_private_dir(parent) {
        return None;
    }
    if let Ok(listener) = UnixListener::bind(sock) {
        return Some(listener);
    }
    if UnixStream::connect(sock).is_ok() {
        return None;
    }
    let _ = std::fs::remove_file(sock);
    UnixListener::bind(sock).ok()
}

/// Accept loop with an idle timeout: `poll` the listener and exit once it sits
/// quiet for `IDLE_MS`, so abandoned per-project daemons don't linger.
fn serve(listener: &UnixListener, index: &Index) {
    use std::os::unix::io::AsRawFd as _;
    let idle_ms = std::env::var("IX_FFF_SUGGEST_IDLE_MS")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(IDLE_MS);
    let fd = listener.as_raw_fd();
    loop {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is a valid, initialized single-element array for `poll`.
        let ready = unsafe { libc::poll(&raw mut pfd, 1, idle_ms) };
        if ready == 0 {
            return; // idle timeout
        }
        if ready < 0 {
            continue; // EINTR and friends; retry
        }
        if let Ok((stream, _)) = listener.accept() {
            handle_conn(stream, index);
        }
    }
}

fn handle_conn(mut stream: UnixStream, index: &Index) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    // Read one request line (the query).
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => buf.push(byte[0]),
            Err(_) => return,
        }
    }
    let query = String::from_utf8_lossy(&buf);
    let mut reply = String::new();
    for path in index.search(query.trim(), SEARCH_LIMIT) {
        reply.push_str(&path);
        reply.push('\n');
    }
    let _ = stream.write_all(reply.as_bytes());
}

// ── libfff_c binding ─────────────────────────────────────────────────────────
//
// We drive the same stable C ABI the notebook kernel binds via ctypes
// (`packages/mcp/src/fff/fff/__init__.py`). Only the file-search slice is bound.

/// Options for `fff_create_instance_with`. `#[repr(C)]`, layout asserted stable
/// by fff-c (88 bytes); fields only ever appended, so v1 stays valid.
#[repr(C)]
struct FffCreateOptions {
    version: u32,
    base_path: *const c_char,
    frecency_db_path: *const c_char,
    history_db_path: *const c_char,
    enable_mmap_cache: bool,
    enable_content_indexing: bool,
    watch: bool,
    ai_mode: bool,
    log_file_path: *const c_char,
    log_level: *const c_char,
    cache_budget_max_files: u64,
    cache_budget_max_bytes: u64,
    cache_budget_max_file_size: u64,
    enable_fs_root_scanning: bool,
    enable_home_dir_scanning: bool,
}

/// The envelope every `fff_*` call returns by pointer. `handle` carries the
/// typed payload; `int_value` carries scalar returns.
#[repr(C)]
struct FffResult {
    success: bool,
    error: *const c_char,
    handle: *mut c_void,
    int_value: i64,
}

type CreateFn = unsafe extern "C" fn(*const FffCreateOptions) -> *mut FffResult;
type DestroyFn = unsafe extern "C" fn(*mut c_void);
type WaitFn = unsafe extern "C" fn(*mut c_void, u64) -> *mut FffResult;
type SearchFn =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, u32, u32, u32, i32, u32) -> *mut FffResult;
type FreeResultFn = unsafe extern "C" fn(*mut FffResult);
type FreeSearchFn = unsafe extern "C" fn(*mut c_void);
type CountFn = unsafe extern "C" fn(*const c_void) -> u32;
type ItemFn = unsafe extern "C" fn(*const c_void, u32) -> *const c_void;
type RelPathFn = unsafe extern "C" fn(*const c_void) -> *const c_char;

/// The dlopen'd symbols, kept alive for the daemon's lifetime. The `Library` is
/// leaked so the borrowed `Symbol`s are `'static`; the process owns it until
/// exit, which reclaims it.
struct Fff {
    create: Symbol<'static, CreateFn>,
    destroy: Symbol<'static, DestroyFn>,
    wait: Symbol<'static, WaitFn>,
    search: Symbol<'static, SearchFn>,
    free_result: Symbol<'static, FreeResultFn>,
    free_search: Symbol<'static, FreeSearchFn>,
    count: Symbol<'static, CountFn>,
    item: Symbol<'static, ItemFn>,
    rel_path: Symbol<'static, RelPathFn>,
}

impl Fff {
    fn load() -> Result<Self, String> {
        let path = std::env::var_os("IX_FFF_LIB")
            .ok_or_else(|| "IX_FFF_LIB is unset (no libfff_c path baked in)".to_owned())?;
        // SAFETY: loading the trusted, baked libfff_c; its init runs no
        // attacker-controlled code.
        let lib: &'static Library = Box::leak(Box::new(
            unsafe { Library::new(&path) }
                .map_err(|e| format!("dlopen {}: {e}", Path::new(&path).display()))?,
        ));
        // SAFETY: every symbol below matches the fff-c declaration it is typed
        // against (verified against crates/fff-c/include/fff.h).
        unsafe {
            Ok(Self {
                create: get(lib, b"fff_create_instance_with")?,
                destroy: get(lib, b"fff_destroy")?,
                wait: get(lib, b"fff_wait_for_scan")?,
                search: get(lib, b"fff_search")?,
                free_result: get(lib, b"fff_free_result")?,
                free_search: get(lib, b"fff_free_search_result")?,
                count: get(lib, b"fff_search_result_get_count")?,
                item: get(lib, b"fff_search_result_get_item")?,
                rel_path: get(lib, b"fff_file_item_get_relative_path")?,
            })
        }
    }
}

/// SAFETY: `name` must name a symbol whose true signature is `T`.
unsafe fn get<T>(lib: &'static Library, name: &[u8]) -> Result<Symbol<'static, T>, String> {
    unsafe { lib.get(name) }
        .map_err(|e| format!("missing symbol {}: {e}", String::from_utf8_lossy(name)))
}

/// One resident, watched fff index over a directory tree.
struct Index {
    fff: Fff,
    handle: *mut c_void,
    // Kept alive so `FffCreateOptions.base_path` stays valid for the instance.
    _base: CString,
}

impl Index {
    fn open(root: &Path) -> Result<Self, String> {
        let fff = Fff::load()?;
        let base = CString::new(root.as_os_str().as_encoded_bytes())
            .map_err(|_| "root path contains an interior NUL".to_owned())?;
        let opts = FffCreateOptions {
            version: OPTIONS_VERSION,
            base_path: base.as_ptr(),
            frecency_db_path: std::ptr::null(),
            history_db_path: std::ptr::null(),
            enable_mmap_cache: false,
            // Filename search only: no content index keeps it light and fast.
            enable_content_indexing: false,
            watch: true,
            ai_mode: true,
            log_file_path: std::ptr::null(),
            log_level: std::ptr::null(),
            cache_budget_max_files: 0,
            cache_budget_max_bytes: 0,
            cache_budget_max_file_size: 0,
            enable_fs_root_scanning: false,
            enable_home_dir_scanning: false,
        };
        // SAFETY: `opts` is fully initialized and outlives the call; `base`
        // outlives the returned instance (held in `_base`).
        let res = unsafe { (fff.create)(&raw const opts) };
        let handle = unsafe { take_handle(&fff, res) }?;
        Ok(Self {
            fff,
            handle,
            _base: base,
        })
    }

    fn wait_for_scan(&self, timeout_ms: u64) {
        // SAFETY: `self.handle` is a live instance from `fff_create_instance_with`.
        let res = unsafe { (self.fff.wait)(self.handle, timeout_ms) };
        if !res.is_null() {
            // SAFETY: a non-null result is owned by us; free the envelope only.
            unsafe { (self.fff.free_result)(res) };
        }
    }

    /// Fuzzy file search, frecency-ranked, returning relative paths in rank order.
    fn search(&self, query: &str, limit: u32) -> Vec<String> {
        let Ok(cquery) = CString::new(query) else {
            return Vec::new();
        };
        // SAFETY: live instance, valid NUL-terminated query, null current_file;
        // trailing args mirror the kernel binding (no combo boost).
        let res = unsafe {
            (self.fff.search)(self.handle, cquery.as_ptr(), std::ptr::null(), 0, 0, limit, 0, 0)
        };
        if res.is_null() {
            return Vec::new();
        }
        // SAFETY: `res` is a valid, owned `*mut FffResult`.
        let result = unsafe { &*res };
        let mut out = Vec::new();
        if result.success && !result.handle.is_null() {
            let sr = result.handle;
            // SAFETY: `sr` is a valid `FffSearchResult` until we free it below.
            let count = unsafe { (self.fff.count)(sr) };
            for i in 0..count {
                // SAFETY: `i < count`, so the item pointer is valid (or null).
                let item = unsafe { (self.fff.item)(sr, i) };
                if item.is_null() {
                    continue;
                }
                // SAFETY: `item` is a valid `FffFileItem`; the returned C string
                // is owned by `sr` and valid until `free_search`.
                let ptr = unsafe { (self.fff.rel_path)(item) };
                if !ptr.is_null() {
                    out.push(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned());
                }
            }
            // SAFETY: `sr` came from this result and is freed exactly once.
            unsafe { (self.fff.free_search)(sr) };
        }
        // SAFETY: `res` is freed exactly once after we are done reading it.
        unsafe { (self.fff.free_result)(res) };
        out
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        // SAFETY: `self.handle` is a live instance, destroyed exactly once.
        unsafe { (self.fff.destroy)(self.handle) };
    }
}

/// Read the instance/handle out of a create-style `FffResult`, freeing the
/// envelope (which never owns the handle). Errors carry fff-c's message.
unsafe fn take_handle(fff: &Fff, res: *mut FffResult) -> Result<*mut c_void, String> {
    if res.is_null() {
        return Err("fff returned a null result".to_owned());
    }
    // SAFETY: caller passes a valid, owned `*mut FffResult`.
    let result = unsafe { &*res };
    let success = result.success;
    let handle = result.handle;
    let error = if result.error.is_null() {
        None
    } else {
        // SAFETY: non-null error is a valid C string owned by the envelope.
        Some(unsafe { CStr::from_ptr(result.error) }.to_string_lossy().into_owned())
    };
    // SAFETY: free the envelope only; the handle (when present) stays alive.
    unsafe { (fff.free_result)(res) };
    if success {
        Ok(handle)
    } else {
        Err(error.unwrap_or_else(|| "unknown fff error".to_owned()))
    }
}
