//! End-to-end proxy behavior against local TCP fixtures: a CONNECT tunnel,
//! an absolute-form plain-HTTP request, and a refused upstream all yield the
//! records CI's report is built from.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use net_trace::proxy::{Connection, Recorder, Scheme, spawn};

/// One-shot upstream: accepts a single connection, reads until the request
/// pattern arrives, writes `reply`, and closes.
fn one_shot_upstream(reply: &'static [u8]) -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind upstream");
    let port = listener.local_addr().expect("upstream addr").port();
    thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept upstream");
        let mut buffer = [0_u8; 1024];
        let _ = socket.read(&mut buffer).expect("read upstream request");
        socket.write_all(reply).expect("write upstream reply");
    });
    port
}

fn recorded(recorder: &Recorder) -> Vec<Connection> {
    // Handlers push on close; poll briefly instead of a fixed generous sleep.
    for _ in 0..50 {
        let snapshot = recorder.snapshot();
        if !snapshot.is_empty() {
            return snapshot;
        }
        thread::sleep(Duration::from_millis(20));
    }
    recorder.snapshot()
}

#[test]
fn connect_tunnel_records_host_and_bytes() {
    let upstream_port = one_shot_upstream(b"pong");
    let recorder = Arc::new(Recorder::new());
    let proxy_port = spawn(Arc::clone(&recorder)).expect("spawn proxy");

    let mut client =
        TcpStream::connect((Ipv4Addr::LOCALHOST, proxy_port)).expect("connect proxy");
    client
        .write_all(format!("CONNECT 127.0.0.1:{upstream_port} HTTP/1.1\r\n\r\n").as_bytes())
        .expect("send CONNECT");
    let mut response = [0_u8; 39];
    client.read_exact(&mut response).expect("read 200");
    assert!(response.starts_with(b"HTTP/1.1 200"));
    client.write_all(b"ping").expect("send payload");
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).expect("read tunnel reply");
    assert_eq!(reply, b"pong");
    drop(client);

    let connections = recorded(&recorder);
    assert_eq!(connections.len(), 1);
    let connection = &connections[0];
    assert_eq!(connection.host, "127.0.0.1");
    assert_eq!(connection.port, upstream_port);
    assert_eq!(connection.scheme, Scheme::Connect);
    assert_eq!(connection.bytes_up, 4);
    assert_eq!(connection.bytes_down, 4);
    assert!(!connection.failed);
}

#[test]
fn absolute_form_http_is_forwarded_verbatim() {
    let upstream_port = one_shot_upstream(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi");
    let recorder = Arc::new(Recorder::new());
    let proxy_port = spawn(Arc::clone(&recorder)).expect("spawn proxy");

    let mut client =
        TcpStream::connect((Ipv4Addr::LOCALHOST, proxy_port)).expect("connect proxy");
    client
        .write_all(
            format!(
                "GET http://127.0.0.1:{upstream_port}/x HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"
            )
            .as_bytes(),
        )
        .expect("send absolute-form GET");
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).expect("read response");
    assert!(reply.ends_with(b"hi"));
    drop(client);

    let connections = recorded(&recorder);
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0].scheme, Scheme::Http);
    assert_eq!(connections[0].port, upstream_port);
    assert!(connections[0].bytes_up > 0);
    assert!(connections[0].bytes_down > 0);
}

#[test]
fn refused_upstream_is_recorded_as_failed() {
    // Bind-then-drop: the port existed, so nothing else grabs it instantly,
    // and connecting to it now gets refused.
    let dead_port = {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind throwaway");
        listener.local_addr().expect("throwaway addr").port()
    };
    let recorder = Arc::new(Recorder::new());
    let proxy_port = spawn(Arc::clone(&recorder)).expect("spawn proxy");

    let mut client =
        TcpStream::connect((Ipv4Addr::LOCALHOST, proxy_port)).expect("connect proxy");
    client
        .write_all(format!("CONNECT 127.0.0.1:{dead_port} HTTP/1.1\r\n\r\n").as_bytes())
        .expect("send CONNECT");
    let mut response = Vec::new();
    let _ = client.read_to_end(&mut response);
    assert!(response.starts_with(b"HTTP/1.1 502"));

    let connections = recorded(&recorder);
    assert_eq!(connections.len(), 1);
    assert!(connections[0].failed);
    assert_eq!(connections[0].bytes_down, 0);
}
