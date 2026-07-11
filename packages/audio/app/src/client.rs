//! Client side of the control socket: one JSON line out, one back.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;

use anyhow::{Context as _, Result};

use crate::control::{self, Request, Response};

/// Send one request to the local daemon and return its reply.
pub fn request(request: &Request) -> Result<Response> {
    let path = control::socket_path();
    let mut stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "connect {} (is `shared-audio daemon` running?)",
            path.display()
        )
    })?;
    let mut payload = serde_json::to_string(request)?;
    payload.push('\n');
    stream.write_all(payload.as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(serde_json::from_str(&line).context("parse daemon reply")?)
}

/// Send a request, pretty-print the reply, and fail on daemon errors.
pub fn run(request: &Request) -> Result<()> {
    let response = self::request(request)?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    anyhow::ensure!(response.ok, "daemon refused the request");
    Ok(())
}
