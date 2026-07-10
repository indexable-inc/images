//! Platform-independent HTTP/1.1 request handling shared by every
//! metal-httpd backend.
//!
//! The whole point of this crate is the `#![no_std]` at the top: the same
//! request parser and response writer runs as a normal Linux process, under
//! Eyra's pure-Rust libc, inside the Hermit unikernel, and in the
//! freestanding kernel. Backends only differ in how bytes arrive (BSD
//! sockets vs. smoltcp over virtio-net), so everything above the transport
//! lives here.
//!
//! No allocation: responses are rendered into a caller-provided buffer so
//! the bare-metal backend can use it before a heap exists.
#![no_std]

use core::fmt::Write as _;

/// Longest response `render_response` can produce. Callers hand us at least
/// this many bytes so rendering never needs to allocate or truncate.
pub const MAX_RESPONSE_LEN: usize = 512;

/// A parsed HTTP/1.1 request line.
#[derive(Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
}

/// Returns true once `buf` holds a complete request head (terminating blank
/// line seen). GET requests have no body, so this is the "whole request
/// received" test for every backend.
#[must_use]
pub fn request_complete(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n")
}

/// Parses the request line out of a raw request head.
///
/// Returns `None` for anything that is not `METHOD SP PATH SP VERSION`; the
/// caller answers those with `400 Bad Request`.
#[must_use]
pub fn parse_request(buf: &[u8]) -> Option<Request<'_>> {
    let head = core::str::from_utf8(buf).ok()?;
    let line = head.lines().next()?;
    let mut parts = line.split(' ');
    let method = parts.next().filter(|m| !m.is_empty())?;
    let path = parts.next().filter(|p| p.starts_with('/'))?;
    let version = parts.next()?;
    if !version.starts_with("HTTP/") || parts.next().is_some() {
        return None;
    }
    Some(Request { method, path })
}

/// Renders the response for a raw request head into `out`, returning the
/// number of bytes written.
///
/// `backend` names the platform flavor serving the request (`linux`, `eyra`,
/// `hermit`, `bare-metal`); it is echoed in the body so end-to-end tests can
/// prove which image actually answered.
///
/// # Panics
///
/// Panics if `out` is shorter than [`MAX_RESPONSE_LEN`].
pub fn render_response(request: &[u8], backend: &str, out: &mut [u8]) -> usize {
    assert!(out.len() >= MAX_RESPONSE_LEN, "response buffer too small");
    let mut w = SliceWriter { buf: out, len: 0 };
    match parse_request(request) {
        Some(Request { method: "GET", path }) => {
            // Body first so Content-Length is exact without a second pass.
            let mut body = [0_u8; 256];
            let mut bw = SliceWriter {
                buf: &mut body,
                len: 0,
            };
            let _ = write!(bw, "hello from metal-httpd\nbackend: {backend}\npath: {path}\n");
            let body_len = bw.len;
            let _ = write!(
                w,
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {body_len}\r\nconnection: close\r\n\r\n"
            );
            let body = &body[..body_len];
            w.buf[w.len..w.len + body.len()].copy_from_slice(body);
            w.len += body.len();
        }
        Some(_) => {
            let _ = write!(
                w,
                "HTTP/1.1 405 Method Not Allowed\r\nallow: GET\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
        }
        None => {
            let _ = write!(
                w,
                "HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
        }
    }
    w.len
}

/// `core::fmt::Write` into a fixed byte slice. Writes past the end are
/// reported as `fmt::Error`; sizes are chosen so that never happens for the
/// responses above.
struct SliceWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for SliceWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let end = self.len.checked_add(bytes.len()).ok_or(core::fmt::Error)?;
        if end > self.buf.len() {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn respond(req: &[u8]) -> (usize, [u8; MAX_RESPONSE_LEN]) {
        let mut out = [0_u8; MAX_RESPONSE_LEN];
        let n = render_response(req, "test", &mut out);
        (n, out)
    }

    #[test]
    fn parses_get() {
        let req = parse_request(b"GET /health HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
        assert_eq!(
            req,
            Request {
                method: "GET",
                path: "/health"
            }
        );
    }

    #[test]
    fn ok_response_names_backend_and_path() {
        let (n, out) = respond(b"GET /hello HTTP/1.1\r\n\r\n");
        let text = core::str::from_utf8(&out[..n]).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
        assert!(text.contains("backend: test\n"), "{text}");
        assert!(text.contains("path: /hello\n"), "{text}");
        // Content-Length matches the actual body.
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let len: usize = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length: "))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(len, body.len());
    }

    #[test]
    fn non_get_is_405() {
        let (n, out) = respond(b"POST / HTTP/1.1\r\n\r\n");
        assert!(out[..n].starts_with(b"HTTP/1.1 405"));
    }

    #[test]
    fn garbage_is_400() {
        let (n, out) = respond(b"\xff\xfe nonsense");
        assert!(out[..n].starts_with(b"HTTP/1.1 400"));
    }

    #[test]
    fn completion_detection() {
        assert!(!request_complete(b"GET / HTTP/1.1\r\n"));
        assert!(request_complete(b"GET / HTTP/1.1\r\n\r\n"));
        assert!(request_complete(b"GET / HTTP/1.0\n\n"));
    }
}
