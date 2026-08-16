static DROPPED_SESSIONS: AtomicU64 = AtomicU64::new(0);

/// A stateful handle proving destructor semantics: the BEAM collecting
/// the resource (or its owning process dying) runs `Drop`, observable
/// through `dropped_sessions`.
#[unibind::object]
pub struct Session {
    value: Mutex<i64>,
}

impl Session {
    /// Open a session holding `start`.
    #[unibind(constructor)]
    pub fn new(start: i64) -> Self {
        Self {
            value: Mutex::new(start),
        }
    }

    /// The current value.
    pub fn get(&self) -> i64 {
        *self.value.lock().expect("session mutex poisoned")
    }

    /// Add `delta`, returning the new value.
    pub fn add(&self, delta: i64) -> i64 {
        let mut value = self.value.lock().expect("session mutex poisoned");
        *value += delta;
        *value
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        DROPPED_SESSIONS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Sessions dropped so far.
pub fn dropped_sessions() -> u64 {
    DROPPED_SESSIONS.load(Ordering::SeqCst)
}

/// Yield `0..n`, one item per granted credit.
pub fn count(n: u64) -> UniStream<u64> {
    UniStream::new(futures::stream::iter(0..n))
}

/// Yield `n` binaries, proving bytes cross the stream codec.
pub fn count_blobs(n: u64) -> UniStream<Vec<u8>> {
    // A NUL and a high byte in every item: a UTF-8 codec would mangle
    // both, so the assertion in the suite is not just about arity.
    UniStream::new(futures::stream::iter((0..n).map(|index| {
        let mut blob = vec![0_u8, 255];
        blob.extend_from_slice(index.to_string().as_bytes());
        blob
    })))
}

/// Yield `n` records, proving structs cross the stream codec.
pub fn count_samples(n: u64) -> UniStream<Sample> {
    fn sample(index: u64) -> Sample {
        Sample {
            id: index,
            name: format!("sample-{index}"),
            ratio: 0.5,
            tags: vec!["conformance".to_owned()],
            home: None,
        }
    }
    UniStream::new(futures::stream::iter((0..n).map(sample)))
}
