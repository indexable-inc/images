//! Cloudflare executors against a local API stub: create, converge on
//! already-correct live state, update drift in place, and refuse ambiguity.
//!
//! The stub speaks just enough HTTP for `curl` and answers with Cloudflare's
//! response envelope; `CLOUDFLARE_API_BASE` points the executors at it, the
//! same override an operator would never set in production.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    authorization: String,
}

#[derive(Clone)]
struct Route {
    method: &'static str,
    path: String,
    body: String,
}

struct Stub {
    base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Stub {
    fn serve(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                handle(stream, &routes, &log);
            }
        });
        Self { base, requests }
    }

    fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().expect("stub log lock").clone()
    }

    fn mutations(&self) -> Vec<Recorded> {
        self.recorded()
            .into_iter()
            .filter(|r| r.method != "GET")
            .collect()
    }
}

fn handle(stream: TcpStream, routes: &[Route], log: &Arc<Mutex<Vec<Recorded>>>) {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();

    let mut content_length = 0usize;
    let mut authorization = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            content_length = value.parse().expect("curl sends a numeric Content-Length");
        }
        if let Some(value) = line.strip_prefix("Authorization: ") {
            value.clone_into(&mut authorization);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    log.lock().expect("stub log lock").push(Recorded {
        method: method.clone(),
        path: path.clone(),
        authorization,
    });

    let matched = routes
        .iter()
        .find(|route| route.method == method && route.path == path);
    let response_body = matched.map_or_else(
        || format!(r#"{{"success":false,"errors":[{{"code":7000,"message":"stub has no route for {method} {path}"}}],"result":null}}"#),
        |route| route.body.clone(),
    );
    let mut stream = reader.into_inner();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
}

fn envelope(result: &str) -> String {
    format!(r#"{{"success":true,"errors":[],"result":{result}}}"#)
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

fn efx_apply(dir: &Path, stub: &Stub, plan: &str) -> Run {
    std::fs::write(dir.join("plan.json"), plan).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_efx"))
        .args(["apply", "--ir", "plan.json"])
        .current_dir(dir)
        .env("CLOUDFLARE_API_TOKEN", "stub-token")
        .env("CLOUDFLARE_API_BASE", &stub.base)
        .output()
        .expect("efx binary runs");
    Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    }
}

/// zone -> (dns record, workers route) by reference, plus an r2 bucket:
/// the shape of the ported cloudflare stack, minimized.
const STACK: &str = r#"{"effects": [
  {"name": "cloudflare_zone.ix_dev", "kind": "cloudflare.zone", "executor": "cloudflare.zone",
   "inputs": {"account.id": {"literal": "acc-1"}, "name": {"literal": "ix.dev"},
              "type": {"literal": "full"}}},
  {"name": "cloudflare_dns_record.apex", "kind": "cloudflare.dns_record",
   "executor": "cloudflare.dns_record",
   "inputs": {"zone_id": {"ref": {"effect": "cloudflare_zone.ix_dev", "field": "id"}},
              "name": {"literal": "ix.dev"}, "type": {"literal": "A"},
              "content": {"literal": "192.0.2.10"}, "ttl": {"literal": 1},
              "proxied": {"literal": true}}},
  {"name": "cloudflare_r2_bucket.cli", "kind": "cloudflare.r2_bucket",
   "executor": "cloudflare.r2_bucket",
   "inputs": {"account_id": {"literal": "acc-1"}, "name": {"literal": "ix-cli"}}},
  {"name": "cloudflare_workers_route.apex", "kind": "cloudflare.workers_route",
   "executor": "cloudflare.workers_route",
   "inputs": {"zone_id": {"ref": {"effect": "cloudflare_zone.ix_dev", "field": "id"}},
              "pattern": {"literal": "ix.dev/*"}, "script": {"literal": "ix-web"}}}
]}"#;

fn route(method: &'static str, path: &str, body: String) -> Route {
    Route {
        method,
        path: path.to_owned(),
        body,
    }
}

#[test]
fn creates_the_stack_then_caches_it() {
    let stub = Stub::serve(vec![
        route("GET", "/zones?name=ix.dev", envelope("[]")),
        route(
            "POST",
            "/zones",
            envelope(r#"{"id":"zone-1","name":"ix.dev","type":"full"}"#),
        ),
        route(
            "GET",
            "/zones/zone-1/dns_records?name=ix.dev&type=A&per_page=100",
            envelope("[]"),
        ),
        route(
            "POST",
            "/zones/zone-1/dns_records",
            envelope(r#"{"id":"rec-1"}"#),
        ),
        route(
            "POST",
            "/accounts/acc-1/r2/buckets",
            envelope(r#"{"name":"ix-cli"}"#),
        ),
        route("GET", "/zones/zone-1/workers/routes", envelope("[]")),
        route(
            "POST",
            "/zones/zone-1/workers/routes",
            envelope(r#"{"id":"route-1"}"#),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();

    let first = efx_apply(dir.path(), &stub, STACK);
    assert!(first.success, "{}{}", first.stdout, first.stderr);
    assert!(
        first.stdout.contains("4 executed, 0 cached, 0 failed"),
        "{}",
        first.stdout
    );
    let mutations = stub.mutations();
    // Effects within a level run in parallel, so compare the mutation set,
    // not the arrival order.
    let mut mutated: Vec<String> = mutations.iter().map(|r| r.path.clone()).collect();
    mutated.sort();
    assert_eq!(
        mutated,
        vec![
            "/accounts/acc-1/r2/buckets",
            "/zones",
            "/zones/zone-1/dns_records",
            "/zones/zone-1/workers/routes",
        ]
    );
    assert!(
        mutations
            .iter()
            .all(|r| r.authorization == "Bearer stub-token"),
        "every call authenticates with the env token"
    );

    // Second run: the journal answers everything; the API is not touched.
    let before = stub.recorded().len();
    let second = efx_apply(dir.path(), &stub, STACK);
    assert!(second.success, "{}{}", second.stdout, second.stderr);
    assert!(
        second.stdout.contains("0 executed, 4 cached"),
        "{}",
        second.stdout
    );
    assert_eq!(
        stub.recorded().len(),
        before,
        "cached effects make no API calls"
    );
}

#[test]
fn converges_on_live_state_without_mutating() {
    let live_record = r#"[{"id":"rec-9","name":"ix.dev","type":"A","content":"192.0.2.10","ttl":1,"proxied":true}]"#;
    let stub = Stub::serve(vec![
        route(
            "GET",
            "/zones?name=ix.dev",
            envelope(r#"[{"id":"zone-1","name":"ix.dev","type":"full"}]"#),
        ),
        route(
            "GET",
            "/zones/zone-1/dns_records?name=ix.dev&type=A&per_page=100",
            envelope(live_record),
        ),
        // R2 creation is upsert-by-POST; "already exists" is the steady state.
        Route {
            method: "POST",
            path: "/accounts/acc-1/r2/buckets".to_owned(),
            body: r#"{"success":false,"errors":[{"code":10004,"message":"The bucket you tried to create already exists"}],"result":null}"#.to_owned(),
        },
        route(
            "GET",
            "/zones/zone-1/workers/routes",
            envelope(r#"[{"id":"route-7","pattern":"ix.dev/*","script":"ix-web"}]"#),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();

    let run = efx_apply(dir.path(), &stub, STACK);
    assert!(run.success, "{}{}", run.stdout, run.stderr);
    let mutated: Vec<String> = stub.mutations().iter().map(|r| r.path.clone()).collect();
    assert_eq!(
        mutated,
        vec!["/accounts/acc-1/r2/buckets"],
        "only the R2 upsert POSTs; everything else converges via GETs"
    );
}

#[test]
fn updates_a_drifted_record_in_place() {
    let stale = r#"[{"id":"rec-9","name":"ix.dev","type":"A","content":"198.51.100.99","ttl":1,"proxied":true}]"#;
    let stub = Stub::serve(vec![
        route(
            "GET",
            "/zones?name=ix.dev",
            envelope(r#"[{"id":"zone-1","name":"ix.dev","type":"full"}]"#),
        ),
        route(
            "GET",
            "/zones/zone-1/dns_records?name=ix.dev&type=A&per_page=100",
            envelope(stale),
        ),
        route(
            "PUT",
            "/zones/zone-1/dns_records/rec-9",
            envelope(r#"{"id":"rec-9"}"#),
        ),
        route(
            "POST",
            "/accounts/acc-1/r2/buckets",
            envelope(r#"{"name":"ix-cli"}"#),
        ),
        route(
            "GET",
            "/zones/zone-1/workers/routes",
            envelope(r#"[{"id":"route-7","pattern":"ix.dev/*","script":"ix-web"}]"#),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();

    let run = efx_apply(dir.path(), &stub, STACK);
    assert!(run.success, "{}{}", run.stdout, run.stderr);
    assert!(
        stub.recorded()
            .iter()
            .any(|r| r.method == "PUT" && r.path == "/zones/zone-1/dns_records/rec-9"),
        "the stale A record updates in place: {:?}",
        stub.recorded()
    );
}

#[test]
fn refuses_to_guess_among_ambiguous_records() {
    let set = r#"[
      {"id":"mx-1","name":"ix.dev","type":"A","content":"198.51.100.1","ttl":1},
      {"id":"mx-2","name":"ix.dev","type":"A","content":"198.51.100.2","ttl":1}
    ]"#;
    let stub = Stub::serve(vec![
        route(
            "GET",
            "/zones?name=ix.dev",
            envelope(r#"[{"id":"zone-1","name":"ix.dev","type":"full"}]"#),
        ),
        route(
            "GET",
            "/zones/zone-1/dns_records?name=ix.dev&type=A&per_page=100",
            envelope(set),
        ),
        route(
            "POST",
            "/accounts/acc-1/r2/buckets",
            envelope(r#"{"name":"ix-cli"}"#),
        ),
        route(
            "GET",
            "/zones/zone-1/workers/routes",
            envelope(r#"[{"id":"route-7","pattern":"ix.dev/*","script":"ix-web"}]"#),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();

    let run = efx_apply(dir.path(), &stub, STACK);
    assert!(!run.success, "ambiguity must fail the apply");
    assert!(run.stdout.contains("refusing to guess"), "{}", run.stdout);
    assert!(
        !stub
            .recorded()
            .iter()
            .any(|r| r.method == "PUT" || r.path.contains("dns_records/")),
        "no mutation happens on the ambiguous set: {:?}",
        stub.recorded()
    );
}

#[test]
fn missing_token_fails_before_any_call() {
    let stub = Stub::serve(vec![]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("plan.json"), STACK).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_efx"))
        .args(["apply", "--ir", "plan.json"])
        .current_dir(dir.path())
        .env_remove("CLOUDFLARE_API_TOKEN")
        .env("CLOUDFLARE_API_BASE", &stub.base)
        .output()
        .expect("efx binary runs");
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("CLOUDFLARE_API_TOKEN is not set"),
        "{stdout}"
    );
    assert!(stub.recorded().is_empty(), "no API call without a token");
}
