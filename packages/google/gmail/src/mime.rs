//! RFC 5322 + MIME builder for outgoing Gmail messages.
//!
//! The Gmail API takes a base64url-encoded RFC 5322 message in
//! [`crate::model::Message::raw`]; this module produces that byte string
//! from a typed [`OutgoingMessage`]. Construction is deliberately strict:
//! every header value is checked for bare newlines and ASCII control
//! characters before it lands in the buffer, so a user-supplied subject
//! cannot smuggle additional headers into the message (header-injection).
//!
//! Layout decisions:
//! - text-only or html-only body: a single leaf part, no boundary.
//! - text + html: `multipart/alternative` with both leaves.
//! - any body + attachments: `multipart/mixed` carrying the body part (or
//!   nested `multipart/alternative`) followed by one attachment per leaf.
//!
//! Boundary strings come from a v4 UUID hex, with a short prefix so they
//! visibly mark "this is a MIME boundary" in the wire bytes.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as BASE64_STD, URL_SAFE_NO_PAD};
use uuid::Uuid;

use crate::Result;
use crate::error::UnsafeHeaderSnafu;
use crate::model::{Attachment, OutgoingMessage};

/// Build a Gmail-ready raw payload (URL-safe base64, no padding) for
/// `message`.
///
/// # Errors
/// Returns [`Error::UnsafeHeader`] if any header value contains a bare
/// newline or an ASCII control character.
pub fn build_raw(message: &OutgoingMessage) -> Result<String> {
    let bytes = build_rfc5322(message)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn build_rfc5322(message: &OutgoingMessage) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(1024);
    write_address_list(&mut out, "To", &message.to)?;
    if !message.cc.is_empty() {
        write_address_list(&mut out, "Cc", &message.cc)?;
    }
    if !message.bcc.is_empty() {
        write_address_list(&mut out, "Bcc", &message.bcc)?;
    }
    write_header(&mut out, "Subject", &message.subject)?;
    out.extend_from_slice(b"MIME-Version: 1.0\r\n");

    let layout = Layout::pick(message);
    layout.write_body(&mut out, message)?;
    Ok(out)
}

/// Which MIME layout an outgoing message needs, decided once from its parts.
enum Layout {
    /// One leaf: text/plain or text/html.
    LeafText { html: bool, body: String },
    /// `multipart/alternative` for text + html, no attachments.
    Alternative { text: String, html: String },
    /// `multipart/mixed`: a primary body (leaf or alternative) followed by
    /// one attachment per leaf.
    Mixed {
        primary: PrimaryPart,
        attachments: Vec<Attachment>,
    },
}

enum PrimaryPart {
    LeafText { html: bool, body: String },
    Alternative { text: String, html: String },
}

impl Layout {
    fn pick(message: &OutgoingMessage) -> Self {
        let text = message.body_text.clone();
        let html = message.body_html.clone();
        if message.attachments.is_empty() {
            return match (text, html) {
                (Some(text), Some(html)) => Self::Alternative { text, html },
                (Some(text), None) => Self::LeafText {
                    html: false,
                    body: text,
                },
                (None, Some(html)) => Self::LeafText {
                    html: true,
                    body: html,
                },
                // Empty body is allowed by the wire; default to an empty
                // text/plain so the call still produces a valid message.
                (None, None) => Self::LeafText {
                    html: false,
                    body: String::new(),
                },
            };
        }

        let primary = match (text, html) {
            (Some(text), Some(html)) => PrimaryPart::Alternative { text, html },
            (Some(text), None) => PrimaryPart::LeafText {
                html: false,
                body: text,
            },
            (None, Some(html)) => PrimaryPart::LeafText {
                html: true,
                body: html,
            },
            (None, None) => PrimaryPart::LeafText {
                html: false,
                body: String::new(),
            },
        };
        Self::Mixed {
            primary,
            attachments: message.attachments.clone(),
        }
    }

    fn write_body(self, out: &mut Vec<u8>, _message: &OutgoingMessage) -> Result<()> {
        match self {
            Self::LeafText { html, body } => {
                write_content_type(out, html);
                write_blank_line(out);
                write_body_text(out, &body);
                Ok(())
            }
            Self::Alternative { text, html } => {
                write_alternative(out, &text, &html);
                Ok(())
            }
            Self::Mixed {
                primary,
                attachments,
            } => write_mixed(out, primary, &attachments),
        }
    }
}

fn write_content_type(out: &mut Vec<u8>, html: bool) {
    out.extend_from_slice(if html {
        b"Content-Type: text/html; charset=UTF-8\r\n"
    } else {
        b"Content-Type: text/plain; charset=UTF-8\r\n"
    });
    out.extend_from_slice(b"Content-Transfer-Encoding: 8bit\r\n");
}

fn write_blank_line(out: &mut Vec<u8>) {
    out.extend_from_slice(b"\r\n");
}

fn write_body_text(out: &mut Vec<u8>, body: &str) {
    // Bare LFs in the body get converted to CRLF on the wire; the contents
    // are otherwise opaque (8bit transfer encoding). Done here, not on the
    // typed input, so a caller passing through bytes from a file with mixed
    // line endings does not have to know to normalize first.
    for ch in body.chars() {
        if ch == '\n' && !out.ends_with(b"\r") {
            out.extend_from_slice(b"\r\n");
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
}

fn boundary(prefix: &str) -> String {
    format!("=_{}_{}", prefix, Uuid::new_v4().simple())
}

fn write_alternative(out: &mut Vec<u8>, text: &str, html: &str) {
    let bound = boundary("alt");
    out.extend_from_slice(
        format!("Content-Type: multipart/alternative; boundary=\"{bound}\"\r\n").as_bytes(),
    );
    write_blank_line(out);

    write_boundary(out, &bound, false);
    write_content_type(out, false);
    write_blank_line(out);
    write_body_text(out, text);

    write_boundary(out, &bound, false);
    write_content_type(out, true);
    write_blank_line(out);
    write_body_text(out, html);

    write_boundary(out, &bound, true);
}

fn write_mixed(out: &mut Vec<u8>, primary: PrimaryPart, attachments: &[Attachment]) -> Result<()> {
    let bound = boundary("mix");
    out.extend_from_slice(
        format!("Content-Type: multipart/mixed; boundary=\"{bound}\"\r\n").as_bytes(),
    );
    write_blank_line(out);

    write_boundary(out, &bound, false);
    match primary {
        PrimaryPart::LeafText { html, body } => {
            write_content_type(out, html);
            write_blank_line(out);
            write_body_text(out, &body);
        }
        PrimaryPart::Alternative { text, html } => {
            // Nested multipart/alternative as the primary part. It writes
            // its own opening and closing boundaries inside the body.
            let inner = boundary("alt");
            out.extend_from_slice(
                format!("Content-Type: multipart/alternative; boundary=\"{inner}\"\r\n").as_bytes(),
            );
            write_blank_line(out);

            write_boundary(out, &inner, false);
            write_content_type(out, false);
            write_blank_line(out);
            write_body_text(out, &text);

            write_boundary(out, &inner, false);
            write_content_type(out, true);
            write_blank_line(out);
            write_body_text(out, &html);

            write_boundary(out, &inner, true);
        }
    }

    for attachment in attachments {
        write_boundary(out, &bound, false);
        write_attachment(out, attachment)?;
    }
    write_boundary(out, &bound, true);
    Ok(())
}

fn write_attachment(out: &mut Vec<u8>, attachment: &Attachment) -> Result<()> {
    check_safe("Content-Disposition", &attachment.filename)?;
    check_safe("Content-Type", &attachment.content_type)?;
    out.extend_from_slice(
        format!(
            "Content-Type: {ct}; name=\"{name}\"\r\n",
            ct = attachment.content_type,
            name = attachment.filename,
        )
        .as_bytes(),
    );
    out.extend_from_slice(
        format!(
            "Content-Disposition: attachment; filename=\"{name}\"\r\n",
            name = attachment.filename,
        )
        .as_bytes(),
    );
    out.extend_from_slice(b"Content-Transfer-Encoding: base64\r\n");
    write_blank_line(out);

    // 76-character lines per RFC 2045 §6.8.
    let encoded = BASE64_STD.encode(&attachment.content);
    for chunk in encoded.as_bytes().chunks(76) {
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
    }
    Ok(())
}

fn write_boundary(out: &mut Vec<u8>, bound: &str, terminal: bool) {
    if terminal {
        out.extend_from_slice(format!("--{bound}--\r\n").as_bytes());
    } else {
        out.extend_from_slice(format!("--{bound}\r\n").as_bytes());
    }
}

fn write_header(out: &mut Vec<u8>, name: &'static str, value: &str) -> Result<()> {
    check_safe(name, value)?;
    out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    Ok(())
}

fn write_address_list(out: &mut Vec<u8>, name: &'static str, list: &[String]) -> Result<()> {
    if list.is_empty() {
        return Ok(());
    }
    for address in list {
        check_safe(name, address)?;
    }
    let joined = list.join(", ");
    out.extend_from_slice(format!("{name}: {joined}\r\n").as_bytes());
    Ok(())
}

/// Reject header values that would let a caller smuggle additional headers
/// into the message: bare CR/LF, NUL, or any other ASCII control character.
fn check_safe(name: &'static str, value: &str) -> Result<()> {
    if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return UnsafeHeaderSnafu { header: name }.fail();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn decode_raw(raw: &str) -> String {
        let bytes = URL_SAFE_NO_PAD.decode(raw).expect("base64url decodes");
        String::from_utf8(bytes).expect("text in test")
    }

    #[test]
    fn text_only_message_has_no_multipart_envelope() {
        let raw = build_raw(&OutgoingMessage {
            to: vec!["a@example.com".to_owned()],
            subject: "Hi".to_owned(),
            body_text: Some("body".to_owned()),
            ..OutgoingMessage::default()
        })
        .expect("builds");
        let decoded = decode_raw(&raw);
        assert!(decoded.contains("To: a@example.com\r\n"));
        assert!(decoded.contains("Subject: Hi\r\n"));
        assert!(decoded.contains("Content-Type: text/plain"));
        assert!(decoded.contains("\r\n\r\nbody\r\n"));
        assert!(!decoded.contains("multipart"));
    }

    #[test]
    fn text_plus_html_emits_a_multipart_alternative() {
        let raw = build_raw(&OutgoingMessage {
            to: vec!["a@example.com".to_owned()],
            subject: "Hi".to_owned(),
            body_text: Some("plain".to_owned()),
            body_html: Some("<p>html</p>".to_owned()),
            ..OutgoingMessage::default()
        })
        .expect("builds");
        let decoded = decode_raw(&raw);
        assert!(decoded.contains("multipart/alternative"));
        assert!(decoded.contains("text/plain"));
        assert!(decoded.contains("text/html"));
        assert!(decoded.contains("plain"));
        assert!(decoded.contains("<p>html</p>"));
    }

    #[test]
    fn attachment_emits_a_multipart_mixed_with_base64_payload() {
        let raw = build_raw(&OutgoingMessage {
            to: vec!["a@example.com".to_owned()],
            subject: "Hi".to_owned(),
            body_text: Some("body".to_owned()),
            attachments: vec![Attachment {
                filename: "note.txt".to_owned(),
                content_type: "text/plain".to_owned(),
                content: b"attached payload".to_vec(),
            }],
            ..OutgoingMessage::default()
        })
        .expect("builds");
        let decoded = decode_raw(&raw);
        assert!(decoded.contains("multipart/mixed"));
        assert!(decoded.contains("Content-Disposition: attachment; filename=\"note.txt\""));
        // Encoded body must round-trip back to the input.
        let line = decoded
            .lines()
            .find(|line| line.starts_with("YXR0YWNoZWQ"))
            .expect("base64-encoded attachment line is present");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(line)
            .expect("decodes");
        assert_eq!(bytes, b"attached payload");
    }

    #[test]
    fn header_injection_in_subject_is_refused() {
        let err = build_raw(&OutgoingMessage {
            to: vec!["a@example.com".to_owned()],
            subject: "Hi\r\nBcc: secret@example.com".to_owned(),
            body_text: Some("body".to_owned()),
            ..OutgoingMessage::default()
        })
        .expect_err("rejects");
        assert!(
            matches!(err, Error::UnsafeHeader { header: "Subject" }),
            "got {err:?}"
        );
    }

    #[test]
    fn header_injection_in_address_is_refused() {
        let err = build_raw(&OutgoingMessage {
            to: vec!["a@example.com\r\nBcc: leak@example.com".to_owned()],
            subject: "Hi".to_owned(),
            body_text: Some("body".to_owned()),
            ..OutgoingMessage::default()
        })
        .expect_err("rejects");
        assert!(
            matches!(err, Error::UnsafeHeader { header: "To" }),
            "got {err:?}"
        );
    }

    #[test]
    fn body_lf_endings_are_normalized_to_crlf() {
        let raw = build_raw(&OutgoingMessage {
            to: vec!["a@example.com".to_owned()],
            subject: "Hi".to_owned(),
            body_text: Some("line one\nline two".to_owned()),
            ..OutgoingMessage::default()
        })
        .expect("builds");
        let decoded = decode_raw(&raw);
        assert!(decoded.contains("line one\r\nline two\r\n"));
    }
}
