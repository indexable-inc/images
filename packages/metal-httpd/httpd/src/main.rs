//! metal-httpd, userspace flavors: the linux, eyra, and hermit backends all
//! compile from this file. Transport is std's TCP; everything HTTP is
//! delegated to `http-core`, the same code the freestanding kernel runs.

// Pull in Eyra's libc replacement: linking the crate is what swaps the C
// runtime out (together with -nostartfiles from build.rs).
#[cfg(feature = "eyra")]
extern crate eyra;

// Linking the Hermit kernel into the image is what makes this binary a
// unikernel; std's hermit target calls into it for syscalls.
#[cfg(target_os = "hermit")]
use hermit as _;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

const BACKEND: &str = if cfg!(target_os = "hermit") {
    "hermit"
} else if cfg!(feature = "eyra") {
    "eyra"
} else {
    "linux"
};

fn main() {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080_u16);
    let listener = TcpListener::bind(("0.0.0.0", port))
        .unwrap_or_else(|e| panic!("metal-httpd[{BACKEND}]: bind 0.0.0.0:{port} failed: {e}"));
    println!("metal-httpd[{BACKEND}]: listening on 0.0.0.0:{port}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(e) = handle(stream) {
                    eprintln!("metal-httpd[{BACKEND}]: connection error: {e}");
                }
            }
            Err(e) => eprintln!("metal-httpd[{BACKEND}]: accept error: {e}"),
        }
    }
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    let mut request = [0_u8; 1024];
    let mut filled = 0;
    while !http_core::request_complete(&request[..filled]) {
        if filled == request.len() {
            break; // Oversized head: let http-core answer what we have (400).
        }
        let n = stream.read(&mut request[filled..])?;
        if n == 0 {
            return Ok(()); // Peer went away before finishing the request.
        }
        filled += n;
    }
    let mut response = [0_u8; http_core::MAX_RESPONSE_LEN];
    let len = http_core::render_response(&request[..filled], BACKEND, &mut response);
    stream.write_all(&response[..len])?;
    stream.shutdown(std::net::Shutdown::Both)?;
    Ok(())
}
