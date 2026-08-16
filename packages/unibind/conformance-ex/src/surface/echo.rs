/// Round-trip a bool.
pub fn echo_bool(value: bool) -> bool {
    value
}

/// Round-trip a signed int.
pub fn echo_int(value: i64) -> i64 {
    value
}

/// Round-trip an unsigned int.
pub fn echo_uint(value: u32) -> u32 {
    value
}

/// Round-trip a float.
pub fn echo_float(value: f64) -> f64 {
    value
}

/// Round-trip a string.
pub fn echo_str(value: String) -> String {
    value
}

/// Round-trip an optional string; `nil` crosses as `None`.
pub fn echo_option(value: Option<String>) -> Option<String> {
    value
}

/// Round-trip a list of ints.
pub fn echo_vec(values: Vec<i64>) -> Vec<i64> {
    values
}

/// Round-trip a string-keyed map of ints.
pub fn echo_map(values: HashMap<String, i64>) -> HashMap<String, i64> {
    values
}

/// Round-trip a borrowed byte slice; binaries cross as binaries, not
/// as lists of integers.
pub fn echo_bytes(data: &[u8]) -> Vec<u8> {
    data.to_vec()
}

/// Round-trip an owned byte buffer.
pub fn echo_bytes_owned(data: Vec<u8>) -> Vec<u8> {
    data
}

/// The length of a borrowed slice: proves the argument arrived whole
/// rather than as text, whatever bytes it holds.
pub fn bytes_len(data: &[u8]) -> usize {
    data.len()
}

/// Round-trip an optional binary; `nil` crosses as `None`.
pub fn echo_bytes_option(data: Option<Vec<u8>>) -> Option<Vec<u8>> {
    data
}

/// Round-trip a borrowed optional binary, the other `Option` shape the
/// IR allows in argument position.
pub fn echo_bytes_option_ref(data: Option<&[u8]>) -> Option<Vec<u8>> {
    data.map(<[u8]>::to_vec)
}

/// Round-trip a list of binaries: bytes nested one container deep.
pub fn echo_bytes_list(data: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    data
}

/// Round-trip a string-keyed map of binaries: bytes as a map value.
pub fn echo_bytes_map(data: HashMap<String, Vec<u8>>) -> HashMap<String, Vec<u8>> {
    data
}

/// Binaries through a dirty scheduler, where the wrapper is a sync NIF.
#[unibind(blocking)]
pub fn blocking_bytes(data: &[u8]) -> Vec<u8> {
    data.to_vec()
}

/// Binaries through a `Result`, where the value rides inside `Ok`.
///
/// # Errors
///
/// When `fail` is true.
pub fn maybe_bytes(fail: bool) -> Result<Vec<u8>, ConformanceError> {
    if fail {
        return Err(ConformanceError::Gone {
            message: "conformance bytes failure".to_owned(),
        });
    }
    Ok(vec![0, 255, 128])
}

/// Binaries through the async path, where owned arguments move into a
/// `'static` future.
pub async fn echo_bytes_async(data: Vec<u8>) -> Vec<u8> {
    data
}

/// Round-trip a record struct.
pub fn echo_record(sample: Sample) -> Sample {
    sample
}

/// Round-trip a nested list of records.
pub fn echo_records(samples: Vec<Sample>) -> Vec<Sample> {
    samples
}

/// Ok or the `:deliberate` error variant, by input.
///
/// # Errors
///
/// When `fail` is true; proving the `{:error, struct}` term shape is
/// the point.
pub fn maybe_fail(fail: bool) -> Result<i64, ConformanceError> {
    if fail {
        return Err(ConformanceError::Deliberate {
            message: "conformance deliberate failure".to_owned(),
        });
    }
    Ok(42)
}

/// Always the `:gone` error variant.
///
/// # Errors
///
/// Always; proving the second variant maps to its own atom.
pub fn lost() -> Result<i64, ConformanceError> {
    Err(ConformanceError::Gone {
        message: "conformance gone failure".to_owned(),
    })
}

/// Sleep on a dirty IO scheduler; compiling and returning proves the
/// `DirtyIo` scheduling attribute round-trips through rustler.
#[unibind(blocking)]
pub fn blocking_sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Echo through the shared tokio runtime: the plain async round-trip.
pub async fn echo_async(value: String) -> String {
    value
}

/// Async Ok or the `:deliberate` error variant, by input.
///
/// # Errors
///
/// When `fail` is true; proving the async reply carries
/// `{:error, struct}` too.
pub async fn maybe_fail_async(fail: bool) -> Result<i64, ConformanceError> {
    if fail {
        return Err(ConformanceError::Deliberate {
            message: "conformance deliberate async failure".to_owned(),
        });
    }
    Ok(7)
}

static CANCELLED: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicU64 = AtomicU64::new(0);

/// Cancellation probe: dropped while still armed (the only way the
/// future can end other than running to completion), it bumps
/// `CANCELLED`. A completed call disarms first, so the counter moves
/// only when the caller's exit aborts the in-flight task. (No inherent
/// impl: inside an exported module those are reserved for
/// `#[unibind::object]` types.)
struct CancelGuard {
    armed: bool,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if self.armed {
            CANCELLED.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Sleep `ms` on the runtime holding a `CancelGuard` across the await,
/// then resolve to `ms`. Cancelled, the guard drops armed.
///
/// Bumps `STARTED` on entry: an async body only runs at first poll, so
/// an abort that lands earlier drops the future with the guard never
/// armed. Callers that want to observe cancellation wait for
/// `started_count` to move before killing the caller.
pub async fn slow(ms: u64) -> u64 {
    let mut guard = CancelGuard { armed: true };
    STARTED.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(ms)).await;
    guard.armed = false;
    ms
}

/// Calls of `slow` cancelled so far (armed guard drops).
pub fn cancelled_count() -> u64 {
    CANCELLED.load(Ordering::SeqCst)
}

/// Calls of `slow` whose body has begun executing (first poll reached;
/// the cancel guard is armed from this point on).
pub fn started_count() -> u64 {
    STARTED.load(Ordering::SeqCst)
}
