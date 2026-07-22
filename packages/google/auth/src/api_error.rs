//! Google's standard REST error envelope.
//!
//! Every Google API the repo wraps reports failures as
//! `{"error": {"code": …, "message": …, …}}`. The typed clients (calendar,
//! gmail) share this one extractor so each keeps its `check_status` seam
//! without a local copy of the envelope types (#3911).

use serde::Deserialize;

/// The Google API error envelope: `{"error": {"code": …, "message": …}}`.
#[derive(Deserialize)]
struct ApiErrorBody {
    error: ApiErrorDetail,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// The human message from a Google error body, or the (truncated) raw body
/// when the envelope is absent.
#[must_use]
pub fn api_message(body: &str) -> String {
    serde_json::from_str::<ApiErrorBody>(body).map_or_else(
        |_| {
            let trimmed = body.trim();
            let mut message: String = trimmed.chars().take(500).collect();
            if message.len() < trimmed.len() {
                message.push('…');
            }
            message
        },
        |envelope| envelope.error.message,
    )
}

#[cfg(test)]
mod tests {
    use super::api_message;

    #[test]
    fn api_message_prefers_the_error_envelope() {
        let body = r#"{"error":{"code":403,"message":"Insufficient Permission","status":"PERMISSION_DENIED"}}"#;
        assert_eq!(api_message(body), "Insufficient Permission");
    }

    #[test]
    fn api_message_falls_back_to_the_raw_body() {
        assert_eq!(api_message(" upstream exploded "), "upstream exploded");
    }
}
