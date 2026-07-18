//! The value a run produces: per-stage argv, status, timing, and bounded
//! captures of every stream that flowed through the pipeline.

use std::collections::VecDeque;

use serde::Serialize;

/// Bounded byte capture: an exact byte count plus a head buffer and a tail
/// ring, so a multi-gigabyte stream costs a fixed amount of memory while the
/// interesting ends survive.
#[derive(Debug, Clone)]
pub struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_limit: usize,
    tail_limit: usize,
    total: u64,
}

impl Capture {
    /// A capture keeping at most `limit` bytes (3/4 head, 1/4 tail).
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        let head_limit = limit / 4 * 3;
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            head_limit,
            tail_limit: limit - head_limit,
            total: 0,
        }
    }

    /// Record bytes flowing through the stream.
    pub fn write(&mut self, buf: &[u8]) {
        self.total += buf.len() as u64;
        let head_room = self.head_limit.saturating_sub(self.head.len());
        let (to_head, to_tail) = buf.split_at(head_room.min(buf.len()));
        self.head.extend_from_slice(to_head);
        self.tail.extend(to_tail.iter().copied());
        while self.tail.len() > self.tail_limit {
            self.tail.pop_front();
        }
    }

    /// Exact number of bytes that flowed through, kept or not.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total
    }

    /// True when bytes were dropped between head and tail.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.total > (self.head.len() + self.tail.len()) as u64
    }

    /// The captured stream as text (lossy UTF-8). A truncated capture marks
    /// the gap loudly so the reader never mistakes it for the full stream.
    #[must_use]
    pub fn render(&self) -> String {
        let head = String::from_utf8_lossy(&self.head);
        if self.truncated() {
            let omitted = self.total - (self.head.len() + self.tail.len()) as u64;
            let tail_bytes: Vec<u8> = self.tail.iter().copied().collect();
            let tail = String::from_utf8_lossy(&tail_bytes);
            format!("{head}\n[plumb: {omitted} bytes omitted]\n{tail}")
        } else if self.tail.is_empty() {
            head.into_owned()
        } else {
            let tail_bytes: Vec<u8> = self.tail.iter().copied().collect();
            format!("{head}{}", String::from_utf8_lossy(&tail_bytes))
        }
    }
}

impl Serialize for Capture {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Repr {
            text: String,
            total_bytes: u64,
            truncated: bool,
        }
        Repr {
            text: self.render(),
            total_bytes: self.total,
            truncated: self.truncated(),
        }
        .serialize(serializer)
    }
}

/// One pipeline stage after it ran.
#[derive(Debug, Clone, Serialize)]
pub struct Stage {
    /// Index of the stage across the whole run (the `K` in `$oN_K`).
    pub index: usize,
    /// The argv actually spawned, after expansion (empty for builtins that
    /// take no arguments? never: argv[0] is always present).
    pub argv: Vec<String>,
    /// True when the stage ran in-process as a builtin.
    pub builtin: bool,
    /// Exit status (128+signal for signal deaths, 127 for not found).
    pub status: i32,
    /// Wall time of this stage in milliseconds.
    pub duration_ms: u64,
    /// Captured stdout of this stage. For stage K, this is also exactly what
    /// stage K+1 received on stdin.
    pub stdout: Capture,
    /// Captured stderr of this stage.
    pub stderr: Capture,
    /// True when `2>&1` merged this stage's stderr into its stdout capture.
    pub stderr_merged: bool,
}

/// One executed pipeline (`a | b | c`).
#[derive(Debug, Clone, Serialize)]
pub struct PipelineRun {
    /// The stages, left to right.
    pub stages: Vec<Stage>,
    /// Pipeline status: the last nonzero stage status, else 0 (pipefail).
    pub status: i32,
}

/// The value of one run: everything that happened when a source string was
/// evaluated.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Run id: the `N` in `$oN` / `$eN` / `$sN`.
    pub id: u64,
    /// The source text that was evaluated.
    pub source: String,
    /// Final status of the run (status of the last pipeline that ran).
    pub status: i32,
    /// Executed pipelines in order (connector short-circuiting means a
    /// source pipeline may not appear here).
    pub pipelines: Vec<PipelineRun>,
    /// Reports of `$(...)` command substitutions, in evaluation order.
    pub substitutions: Vec<Self>,
    /// Run ids of `&` background items this run started.
    pub background_started: Vec<u64>,
    /// Wall time of the whole run in milliseconds.
    pub duration_ms: u64,
    /// When the run aborted early (unset variable, failed glob, redirect
    /// error), the error message; the pipelines above still ran.
    pub aborted: Option<String>,
}

impl Report {
    /// The final stdout of the run: the last stage of the last pipeline.
    #[must_use]
    pub fn output(&self) -> String {
        self.pipelines
            .last()
            .and_then(|p| p.stages.last())
            .map(|s| s.stdout.render())
            .unwrap_or_default()
    }

    /// All stages of the run in execution order.
    pub fn stages(&self) -> impl Iterator<Item = &Stage> {
        self.pipelines.iter().flat_map(|p| p.stages.iter())
    }
}

/// Duration in whole milliseconds without a fallible narrowing conversion.
#[must_use]
pub fn duration_millis(duration: std::time::Duration) -> u64 {
    duration
        .as_secs()
        .saturating_mul(1000)
        .saturating_add(u64::from(duration.subsec_millis()))
}

#[cfg(test)]
mod tests {
    use super::Capture;

    #[test]
    fn capture_keeps_head_and_tail() {
        let mut capture = Capture::new(8);
        capture.write(b"0123456789abcdef");
        assert_eq!(capture.total_bytes(), 16);
        assert!(capture.truncated());
        let text = capture.render();
        assert!(text.starts_with("012345"), "{text}");
        assert!(text.ends_with("ef"), "{text}");
        assert!(text.contains("omitted"), "{text}");
    }

    #[test]
    fn capture_small_stream_is_exact() {
        let mut capture = Capture::new(64);
        capture.write(b"hello ");
        capture.write(b"world");
        assert!(!capture.truncated());
        assert_eq!(capture.render(), "hello world");
    }
}
