//! Cloudflare executors: the apply side of the terranix/opentofu
//! replacement.
//!
//! Each executor reconciles one declared resource against the Cloudflare v4
//! API through `curl` (the same transport the old terranix wrappers used).
//! Auth comes from `CLOUDFLARE_API_TOKEN` in the environment — never from an
//! input, so no secret enters the plan or its identity hashes.
//! `CLOUDFLARE_API_BASE` overrides the endpoint (integration tests point it
//! at a local stub); it defaults to the real API.
//!
//! Reconciliation is deliberately conservative: an executor creates a
//! missing resource, updates an unambiguously-identified one, and refuses
//! loudly whenever the live state makes "which record did you mean?" a
//! guess. efx never destroys — a resource that leaves the plan shows up as a
//! journal orphan, and removal stays a human decision.

use std::io::Write as _;
use std::process::{Command, Stdio};

use efx_engine::{ExecuteError, ExecuteRequest, Executor, Outputs, Registry};
use efx_ir::Literal;
use serde_json::{Value as Json, json};

const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";

/// Cloudflare's "bucket already exists" error code: the steady state for an
/// idempotent bucket declaration, not a failure.
const R2_BUCKET_EXISTS: i64 = 10004;

/// Registers every Cloudflare executor under its `cloudflare.*` kind.
pub fn register(registry: &mut Registry) {
    registry.register("cloudflare.zone", Box::new(Zone));
    registry.register("cloudflare.dns_record", Box::new(DnsRecord));
    registry.register("cloudflare.r2_bucket", Box::new(R2Bucket));
    registry.register("cloudflare.workers_route", Box::new(WorkersRoute));
}

// --- HTTP plumbing ---------------------------------------------------------

struct Api {
    base: String,
    token: String,
}

struct HttpResponse {
    status: u16,
    body: String,
}

#[derive(serde::Deserialize)]
struct Envelope {
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    #[serde(default)]
    result: Json,
}

#[derive(serde::Deserialize)]
struct ApiError {
    code: i64,
    message: String,
}

impl Api {
    fn from_env() -> Result<Self, ExecuteError> {
        let token = std::env::var("CLOUDFLARE_API_TOKEN").map_err(|_| {
            ExecuteError::new(
                "CLOUDFLARE_API_TOKEN is not set; cloudflare.* executors read the API \
                 token from the environment (resolve it from your secret store first)",
            )
        })?;
        let base =
            std::env::var("CLOUDFLARE_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_owned());
        Ok(Self { base, token })
    }

    fn http(
        &self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<HttpResponse, ExecuteError> {
        let url = format!("{}{path}", self.base);
        let mut command = Command::new("curl");
        command.args([
            "-sS",
            "--connect-timeout",
            "15",
            "--max-time",
            "120",
            // Small JSON bodies: skip the 100-continue round trip.
            "-H",
            "Expect:",
            "-X",
            method,
            "-H",
            &format!("Authorization: Bearer {}", self.token),
            "-H",
            "Content-Type: application/json",
            "-w",
            "\n%{http_code}",
            &url,
        ]);
        command.stderr(Stdio::piped()).stdout(Stdio::piped());
        if body.is_some() {
            command.args(["--data-binary", "@-"]);
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command
            .spawn()
            .map_err(|err| ExecuteError::new(format!("spawn curl: {err}")))?;
        if let Some(payload) = body {
            let bytes =
                serde_json::to_vec(payload).unwrap_or_else(|_| unreachable!("json serializes"));
            child
                .stdin
                .take()
                .unwrap_or_else(|| unreachable!("stdin was piped"))
                .write_all(&bytes)
                .map_err(|err| ExecuteError::new(format!("write request body: {err}")))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|err| ExecuteError::new(format!("run curl: {err}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExecuteError::new(format!(
                "curl {method} {url} failed ({}): {}",
                output.status,
                stderr.trim()
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let (response_body, status_line) = stdout.rsplit_once('\n').ok_or_else(|| {
            ExecuteError::new(format!(
                "curl {method} {url}: response carried no status line"
            ))
        })?;
        let status: u16 = status_line.trim().parse().map_err(|_| {
            ExecuteError::new(format!(
                "curl {method} {url}: unparseable status `{status_line}`"
            ))
        })?;
        Ok(HttpResponse {
            status,
            body: response_body.to_owned(),
        })
    }

    /// One API call, parsed into Cloudflare's response envelope. HTTP-level
    /// failures and non-envelope bodies are errors; an unsuccessful envelope
    /// is returned as data so callers can inspect error codes.
    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Envelope, ExecuteError> {
        let response = self.http(method, path, body)?;
        serde_json::from_str(&response.body).map_err(|err| {
            ExecuteError::new(format!(
                "cloudflare API {method} {path} returned a non-envelope response \
                 (HTTP {}): {err}: {}",
                response.status,
                truncate(&response.body)
            ))
        })
    }

    /// One API call that must succeed; envelope errors become executor errors.
    fn expect_success(
        &self,
        method: &str,
        path: &str,
        body: Option<&Json>,
    ) -> Result<Json, ExecuteError> {
        let envelope = self.call(method, path, body)?;
        if envelope.success {
            Ok(envelope.result)
        } else {
            Err(envelope_error(method, path, &envelope))
        }
    }
}

fn envelope_error(method: &str, path: &str, envelope: &Envelope) -> ExecuteError {
    let details: Vec<String> = envelope
        .errors
        .iter()
        .map(|e| format!("{} (code {})", e.message, e.code))
        .collect();
    ExecuteError::new(format!(
        "cloudflare API {method} {path} failed: {}",
        if details.is_empty() {
            "no error detail in envelope".to_owned()
        } else {
            details.join("; ")
        }
    ))
}

fn truncate(body: &str) -> String {
    const LIMIT: usize = 300;
    if body.len() <= LIMIT {
        body.to_owned()
    } else {
        let cut = body
            .char_indices()
            .take_while(|(i, _)| *i <= LIMIT)
            .last()
            .map_or(0, |(i, _)| i);
        format!("{}…", &body[..cut])
    }
}

/// Percent-encodes one query-string value (RFC 3986 unreserved set).
fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => {
                let formatted = format!("%{other:02X}");
                encoded.push_str(&formatted);
            }
        }
    }
    encoded
}

// --- Input plumbing --------------------------------------------------------

/// Rejects input keys outside the executor's contract, so a typo (or a
/// terraform attribute with no efx meaning) fails instead of being silently
/// dropped from the payload.
fn check_keys(request: &ExecuteRequest, allowed: &[&str]) -> Result<(), ExecuteError> {
    for key in request.inputs.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ExecuteError::new(format!(
                "`{}` does not understand input `{key}` (allowed: {})",
                request.kind,
                allowed.join(", ")
            )));
        }
    }
    Ok(())
}

fn required_str(request: &ExecuteRequest, key: &str) -> Result<String, ExecuteError> {
    request
        .inputs
        .get(key)
        .map(Literal::display_string)
        .ok_or_else(|| ExecuteError::new(format!("`{}` requires input `{key}`", request.kind)))
}

fn optional_str(request: &ExecuteRequest, key: &str) -> Option<String> {
    request.inputs.get(key).map(Literal::display_string)
}

fn optional_int(request: &ExecuteRequest, key: &str) -> Result<Option<i64>, ExecuteError> {
    match request.inputs.get(key) {
        None => Ok(None),
        Some(Literal::Int(n)) => Ok(Some(*n)),
        Some(other) => Err(ExecuteError::new(format!(
            "`{}` input `{key}` must be an integer, got `{other}`",
            request.kind
        ))),
    }
}

fn optional_bool(request: &ExecuteRequest, key: &str) -> Result<Option<bool>, ExecuteError> {
    match request.inputs.get(key) {
        None => Ok(None),
        Some(Literal::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(ExecuteError::new(format!(
            "`{}` input `{key}` must be a boolean, got `{other}`",
            request.kind
        ))),
    }
}

fn str_field(value: &Json, field: &str) -> Result<String, ExecuteError> {
    value
        .get(field)
        .and_then(Json::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ExecuteError::new(format!(
                "cloudflare API result is missing string field `{field}`: {}",
                truncate(&value.to_string())
            ))
        })
}

fn outcome(id: String, action: &str) -> Outputs {
    Outputs::from([
        ("id".to_owned(), Literal::Str(id)),
        ("action".to_owned(), Literal::Str(action.to_owned())),
    ])
}

// --- cloudflare.zone -------------------------------------------------------

/// `cloudflare.zone`: ensures a zone exists on the account.
///
/// Inputs: `name`, `account.id`, optional `type` (default `full`).
/// Outputs: `id`, `action` (`created` | `unchanged`).
///
/// An existing zone whose type differs from the declaration is an error, not
/// an update: changing a zone's type is a manual, service-affecting move.
struct Zone;

impl Executor for Zone {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        check_keys(request, &["name", "account.id", "type"])?;
        let name = required_str(request, "name")?;
        let account = required_str(request, "account.id")?;
        let zone_type = optional_str(request, "type").unwrap_or_else(|| "full".to_owned());

        let api = Api::from_env()?;
        let listed =
            api.expect_success("GET", &format!("/zones?name={}", encode_query(&name)), None)?;
        let zones = listed.as_array().cloned().unwrap_or_default();
        let existing = zones
            .iter()
            .find(|zone| zone.get("name").and_then(Json::as_str) == Some(name.as_str()));

        if let Some(zone) = existing {
            let live_type = str_field(zone, "type")?;
            if live_type != zone_type {
                return Err(ExecuteError::new(format!(
                    "zone `{name}` exists with type `{live_type}` but the plan declares \
                     `{zone_type}`; refusing to change a zone's type automatically"
                )));
            }
            return Ok(outcome(str_field(zone, "id")?, "unchanged"));
        }

        let created = api.expect_success(
            "POST",
            "/zones",
            Some(&json!({
                "name": name,
                "account": {"id": account},
                "type": zone_type,
            })),
        )?;
        Ok(outcome(str_field(&created, "id")?, "created"))
    }
}

// --- cloudflare.dns_record -------------------------------------------------

/// The desired record, assembled from inputs, plus how to reconcile it.
struct DesiredRecord {
    name: String,
    record_type: String,
    payload: Json,
    /// `content` for content records; the `data` object for CAA-style
    /// records. Two live records with the same identity are duplicates.
    identity: Identity,
    strategy: Strategy,
}

enum Identity {
    Content(String),
    Data(Json),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Strategy {
    /// Default: the (name, type) pair names one record. Create it, update it
    /// in place, and refuse when several live records make that ambiguous.
    Upsert,
    /// For set-typed records (MX exchanges, CAA entries): the declared
    /// content is one member of a set. Create it when its exact identity is
    /// absent; never replace a differing member.
    Ensure,
}

/// `cloudflare.dns_record`: ensures one DNS record.
///
/// Inputs: `zone_id`, `name`, `type`, then `content` or `data.flags` /
/// `data.tag` / `data.value` (CAA), optional `ttl` (default 1 = automatic),
/// `proxied`, `priority` (MX), and `strategy` (`upsert` | `ensure`, default
/// `upsert`). Outputs: `id`, `action` (`created` | `updated` | `unchanged`).
///
/// efx never deletes: replacing a member of a multi-record set (`strategy =
/// "ensure"`) adds the new member and leaves the old one for a human (the
/// journal reports the orphan).
struct DnsRecord;

impl DnsRecord {
    fn desired(request: &ExecuteRequest) -> Result<DesiredRecord, ExecuteError> {
        check_keys(
            request,
            &[
                "zone_id",
                "name",
                "type",
                "content",
                "ttl",
                "proxied",
                "priority",
                "data.flags",
                "data.tag",
                "data.value",
                "strategy",
            ],
        )?;
        let name = required_str(request, "name")?;
        let record_type = required_str(request, "type")?;
        let content = optional_str(request, "content");
        let ttl = optional_int(request, "ttl")?.unwrap_or(1);
        let proxied = optional_bool(request, "proxied")?;
        let priority = optional_int(request, "priority")?;
        let data_flags = optional_int(request, "data.flags")?;
        let data_tag = optional_str(request, "data.tag");
        let data_value = optional_str(request, "data.value");

        let strategy = match optional_str(request, "strategy").as_deref() {
            None | Some("upsert") => Strategy::Upsert,
            Some("ensure") => Strategy::Ensure,
            Some(other) => {
                return Err(ExecuteError::new(format!(
                    "`{}` input `strategy` must be `upsert` or `ensure`, got `{other}`",
                    request.kind
                )));
            }
        };

        let data = if data_flags.is_none() && data_tag.is_none() && data_value.is_none() {
            None
        } else {
            let mut object = serde_json::Map::new();
            if let Some(flags) = data_flags {
                object.insert("flags".to_owned(), json!(flags));
            }
            if let Some(tag) = data_tag {
                object.insert("tag".to_owned(), json!(tag));
            }
            if let Some(value) = data_value {
                object.insert("value".to_owned(), json!(value));
            }
            Some(Json::Object(object))
        };

        let identity = match (&content, &data) {
            (Some(content), None) => Identity::Content(content.clone()),
            (None, Some(data)) => Identity::Data(data.clone()),
            (Some(_), Some(_)) => {
                return Err(ExecuteError::new(
                    "`cloudflare.dns_record` takes `content` or `data.*`, not both",
                ));
            }
            (None, None) => {
                return Err(ExecuteError::new(
                    "`cloudflare.dns_record` requires `content` or `data.*`",
                ));
            }
        };

        let mut payload = serde_json::Map::new();
        payload.insert("name".to_owned(), json!(name));
        payload.insert("type".to_owned(), json!(record_type));
        payload.insert("ttl".to_owned(), json!(ttl));
        if let Some(content) = content {
            payload.insert("content".to_owned(), json!(content));
        }
        if let Some(data) = &data {
            payload.insert("data".to_owned(), data.clone());
        }
        if let Some(proxied) = proxied {
            payload.insert("proxied".to_owned(), json!(proxied));
        }
        if let Some(priority) = priority {
            payload.insert("priority".to_owned(), json!(priority));
        }

        Ok(DesiredRecord {
            name,
            record_type,
            payload: Json::Object(payload),
            identity,
            strategy,
        })
    }
}

fn identity_matches(existing: &Json, identity: &Identity) -> bool {
    match identity {
        Identity::Content(content) => {
            existing.get("content").and_then(Json::as_str) == Some(content.as_str())
        }
        Identity::Data(data) => {
            let Some(live) = existing.get("data") else {
                return false;
            };
            let Some(fields) = data.as_object() else {
                return false;
            };
            fields
                .iter()
                .all(|(key, value)| live.get(key) == Some(value))
        }
    }
}

/// Does the live record already carry every declared payload field?
fn payload_matches(existing: &Json, payload: &Json) -> bool {
    let Some(fields) = payload.as_object() else {
        return false;
    };
    fields.iter().all(|(key, value)| {
        if key == "data" {
            let Some(declared) = value.as_object() else {
                return false;
            };
            declared
                .iter()
                .all(|(k, v)| existing.get("data").and_then(|d| d.get(k)) == Some(v))
        } else {
            existing.get(key) == Some(value)
        }
    })
}

impl Executor for DnsRecord {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        let zone_id = required_str(request, "zone_id")?;
        let desired = Self::desired(request)?;
        let api = Api::from_env()?;

        let listed = api.expect_success(
            "GET",
            &format!(
                "/zones/{zone_id}/dns_records?name={}&type={}&per_page=100",
                encode_query(&desired.name),
                encode_query(&desired.record_type)
            ),
            None,
        )?;
        let records = listed.as_array().cloned().unwrap_or_default();
        let candidates: Vec<&Json> = records
            .iter()
            .filter(|record| identity_matches(record, &desired.identity))
            .collect();

        let create = || {
            api.expect_success(
                "POST",
                &format!("/zones/{zone_id}/dns_records"),
                Some(&desired.payload),
            )
        };
        let update = |id: &str| {
            api.expect_success(
                "PUT",
                &format!("/zones/{zone_id}/dns_records/{id}"),
                Some(&desired.payload),
            )
        };
        let settle = |record: &Json| -> Result<Outputs, ExecuteError> {
            let id = str_field(record, "id")?;
            if payload_matches(record, &desired.payload) {
                Ok(outcome(id, "unchanged"))
            } else {
                update(&id)?;
                Ok(outcome(id, "updated"))
            }
        };

        match (desired.strategy, records.len(), candidates.as_slice()) {
            // Nothing lives under this (name, type): create. Same for a set
            // member (`ensure`) whose exact identity is absent — it is added,
            // and any old members stay, because efx never deletes.
            (_, 0, _) | (Strategy::Ensure, _, []) => {
                Ok(outcome(str_field(&create()?, "id")?, "created"))
            }
            // Exactly one identity match: sync its remaining fields.
            (_, _, [record]) => settle(record),
            // Upsert over a single live record: replace it in place.
            (Strategy::Upsert, 1, []) => settle(&records[0]),
            (Strategy::Upsert, live, []) => Err(ExecuteError::new(format!(
                "{live} records exist for `{}` {} and none matches the declared \
                 content; refusing to guess which to replace. Use `strategy = \
                 \"ensure\"` for set-typed records, or clean up the live set.",
                desired.name, desired.record_type
            ))),
            (_, _, duplicates) => Err(ExecuteError::new(format!(
                "{} live records for `{}` {} carry the declared content; the set \
                 has duplicates that must be cleaned up manually",
                duplicates.len(),
                desired.name,
                desired.record_type
            ))),
        }
    }
}

// --- cloudflare.r2_bucket --------------------------------------------------

/// `cloudflare.r2_bucket`: ensures an R2 bucket exists.
///
/// Inputs: `account_id`, `name`. Outputs: `id` (the bucket name — R2's
/// stable identifier), `action` (`created` | `unchanged`). "Bucket already
/// exists" (code 10004) is the steady state, exactly as the old terranix
/// wrapper treated it.
struct R2Bucket;

impl Executor for R2Bucket {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        check_keys(request, &["account_id", "name"])?;
        let account = required_str(request, "account_id")?;
        let name = required_str(request, "name")?;
        let api = Api::from_env()?;
        let path = format!("/accounts/{account}/r2/buckets");
        let envelope = api.call("POST", &path, Some(&json!({"name": name})))?;
        if envelope.success {
            return Ok(outcome(name, "created"));
        }
        if envelope.errors.iter().any(|e| e.code == R2_BUCKET_EXISTS) {
            return Ok(outcome(name, "unchanged"));
        }
        Err(envelope_error("POST", &path, &envelope))
    }
}

// --- cloudflare.workers_route ----------------------------------------------

/// `cloudflare.workers_route`: ensures a Workers route for a pattern.
///
/// Inputs: `zone_id`, `pattern`, optional `script` (omitted = bypass route:
/// requests matching the pattern skip any worker). Outputs: `id`, `action`
/// (`created` | `updated` | `unchanged`).
struct WorkersRoute;

impl Executor for WorkersRoute {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        check_keys(request, &["zone_id", "pattern", "script"])?;
        let zone_id = required_str(request, "zone_id")?;
        let pattern = required_str(request, "pattern")?;
        let script = optional_str(request, "script");

        let api = Api::from_env()?;
        let listed =
            api.expect_success("GET", &format!("/zones/{zone_id}/workers/routes"), None)?;
        let routes = listed.as_array().cloned().unwrap_or_default();
        let matches: Vec<&Json> = routes
            .iter()
            .filter(|route| route.get("pattern").and_then(Json::as_str) == Some(pattern.as_str()))
            .collect();

        let mut payload = serde_json::Map::new();
        payload.insert("pattern".to_owned(), json!(pattern));
        if let Some(script) = &script {
            payload.insert("script".to_owned(), json!(script));
        }
        let payload = Json::Object(payload);

        match matches.as_slice() {
            [] => {
                let created = api.expect_success(
                    "POST",
                    &format!("/zones/{zone_id}/workers/routes"),
                    Some(&payload),
                )?;
                Ok(outcome(str_field(&created, "id")?, "created"))
            }
            [route] => {
                let id = str_field(route, "id")?;
                let live_script = route
                    .get("script")
                    .and_then(Json::as_str)
                    .map(ToOwned::to_owned);
                if live_script == script {
                    Ok(outcome(id, "unchanged"))
                } else {
                    api.expect_success(
                        "PUT",
                        &format!("/zones/{zone_id}/workers/routes/{id}"),
                        Some(&payload),
                    )?;
                    Ok(outcome(id, "updated"))
                }
            }
            several => Err(ExecuteError::new(format!(
                "{} live workers routes match pattern `{pattern}`; the zone's \
                 route table has duplicates that must be cleaned up manually",
                several.len()
            ))),
        }
    }
}

// --- Tests over the pure reconciliation pieces ------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_matches_content_and_caa_data() {
        let live = json!({"content": "1.2.3.4", "id": "r1"});
        assert!(identity_matches(
            &live,
            &Identity::Content("1.2.3.4".into())
        ));
        assert!(!identity_matches(
            &live,
            &Identity::Content("5.6.7.8".into())
        ));

        let caa = json!({"data": {"flags": 0, "tag": "issue", "value": "pki.goog"}});
        assert!(identity_matches(
            &caa,
            &Identity::Data(json!({"flags": 0, "tag": "issue", "value": "pki.goog"}))
        ));
        assert!(!identity_matches(
            &caa,
            &Identity::Data(json!({"flags": 0, "tag": "issue", "value": "ssl.com"}))
        ));
    }

    #[test]
    fn payload_match_ignores_undeclared_live_fields() {
        let live = json!({
            "id": "r1", "name": "ix.dev", "type": "A", "content": "1.2.3.4",
            "ttl": 1, "proxied": true, "created_on": "2026-01-01",
        });
        let declared = json!({"name": "ix.dev", "type": "A", "content": "1.2.3.4", "ttl": 1});
        assert!(payload_matches(&live, &declared));
        let drifted = json!({"name": "ix.dev", "type": "A", "content": "1.2.3.4", "ttl": 300});
        assert!(!payload_matches(&live, &drifted));
    }

    #[test]
    fn query_encoding_covers_wildcards() {
        assert_eq!(encode_query("*.apps.getix.dev"), "%2A.apps.getix.dev");
        assert_eq!(encode_query("ix.dev"), "ix.dev");
    }
}
