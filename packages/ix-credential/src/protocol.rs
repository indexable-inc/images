//! git's credential line format, used in both directions.
//!
//! `gitcredentials(7)` defines a helper's stdin and stdout as `key=value`
//! lines terminated by a blank line or EOF. This module implements exactly
//! that, and the helper-to-server hop reuses it verbatim rather than
//! inventing a second encoding: one parser, one set of tests, and a server
//! that can be exercised by replaying bytes git actually sent.

use std::io::{BufRead, Write};

use color_eyre::eyre::{Result, bail};

/// A refusal rides back on this key. git has no error channel of its own, so
/// the server answers either credential fields or this one field, and the
/// helper turns it into a diagnostic rather than passing it to git.
pub const ERROR_KEY: &str = "error";

/// Total bytes accepted for one message. The server reads from a socket any
/// local process can reach, so a peer that never sends a blank line must not
/// be able to grow the process instead of being refused.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Fields accepted in one message, for the same reason.
const MAX_FIELDS: usize = 256;

/// One credential message.
///
/// Keys repeat in real traffic (git sends `capability[]` and `wwwauth[]`
/// more than once), so this is an ordered list of pairs and not a map.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Message {
    fields: Vec<(String, String)>,
}

impl Message {
    /// Read one message: `key=value` lines up to a blank line or EOF.
    ///
    /// # Errors
    ///
    /// A line with no `=`, a message past the size or field caps, or an
    /// underlying read failure.
    pub fn read(reader: &mut impl BufRead) -> Result<Self> {
        let mut fields = Vec::new();
        let mut budget = MAX_MESSAGE_BYTES;
        let mut line = String::new();

        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }

            let Some(rest) = budget.checked_sub(line.len()) else {
                bail!("credential message exceeds {MAX_MESSAGE_BYTES} bytes");
            };
            budget = rest;

            let trimmed = line.strip_suffix('\n').unwrap_or(&line);
            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
            if trimmed.is_empty() {
                break;
            }

            let Some((key, value)) = trimmed.split_once('=') else {
                bail!("malformed credential line, no '=': {trimmed:?}");
            };
            if fields.len() == MAX_FIELDS {
                bail!("credential message exceeds {MAX_FIELDS} fields");
            }
            fields.push((key.to_owned(), value.to_owned()));
        }

        Ok(Self { fields })
    }

    /// Write the message and its terminating blank line.
    ///
    /// # Errors
    ///
    /// Any underlying write failure.
    pub fn write(&self, writer: &mut impl Write) -> Result<()> {
        for (key, value) in &self.fields {
            writeln!(writer, "{key}={value}")?;
        }
        writeln!(writer)?;
        writer.flush()?;
        Ok(())
    }

    /// The first value for `key`, if the message carries one.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// Append a field. Values carrying a newline would forge a field
    /// boundary on the wire, so they are refused rather than escaped.
    ///
    /// # Errors
    ///
    /// A key or value containing `\n`, `\r` or NUL, or a key containing `=`.
    pub fn push(&mut self, key: &str, value: &str) -> Result<()> {
        let forbidden = ['\n', '\r', '\0'];
        if key.contains(forbidden) || key.contains('=') {
            bail!("credential key {key:?} contains a separator");
        }
        if value.contains(forbidden) {
            bail!("credential value for {key:?} contains a line break");
        }
        self.fields.push((key.to_owned(), value.to_owned()));
        Ok(())
    }

    /// A message carrying only a refusal.
    ///
    /// # Errors
    ///
    /// A `reason` containing a line break.
    pub fn refusal(reason: &str) -> Result<Self> {
        let mut message = Self::default();
        message.push(ERROR_KEY, reason)?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Result<Message> {
        Message::read(&mut input.as_bytes())
    }

    #[test]
    fn reads_a_real_git_get_request() {
        // Captured from git 2.x on a fleet host talking to github.com.
        let message = parse(
            "capability[]=authtype\n\
             capability[]=state\n\
             protocol=https\n\
             host=github.com\n\
             \n",
        )
        .expect("parses");

        assert_eq!(message.get("protocol"), Some("https"));
        assert_eq!(message.get("host"), Some("github.com"));
        // A repeated key keeps its first value rather than collapsing.
        assert_eq!(message.get("capability[]"), Some("authtype"));
    }

    #[test]
    fn stops_at_the_blank_line_and_leaves_the_rest() {
        let input = "host=github.com\n\nhost=evil.example\n";
        let mut reader = input.as_bytes();
        let first = Message::read(&mut reader).expect("parses");
        assert_eq!(first.get("host"), Some("github.com"));

        let second = Message::read(&mut reader).expect("parses");
        assert_eq!(second.get("host"), Some("evil.example"));
    }

    #[test]
    fn eof_terminates_a_message_without_a_blank_line() {
        let message = parse("protocol=https\nhost=github.com").expect("parses");
        assert_eq!(message.get("host"), Some("github.com"));
    }

    #[test]
    fn an_empty_value_is_a_value() {
        let message = parse("username=\n\n").expect("parses");
        assert_eq!(message.get("username"), Some(""));
    }

    #[test]
    fn a_value_may_contain_equals() {
        let message = parse("wwwauth[]=Basic realm=\"GitHub\"\n\n").expect("parses");
        assert_eq!(message.get("wwwauth[]"), Some("Basic realm=\"GitHub\""));
    }

    #[test]
    fn a_line_without_equals_is_refused() {
        let error = parse("garbage\n\n").expect_err("refused");
        assert!(format!("{error}").contains("no '='"), "{error}");
    }

    #[test]
    fn an_unterminated_flood_is_refused_rather_than_buffered() {
        let flood = "k=v\n".repeat(MAX_MESSAGE_BYTES);
        let error = parse(&flood).expect_err("refused");
        assert!(format!("{error}").contains("exceeds"), "{error}");
    }

    #[test]
    fn round_trips_through_write() {
        let mut message = Message::default();
        message.push("username", "x-access-token").expect("valid");
        message.push("password", "s3cret").expect("valid");

        let mut buffer = Vec::new();
        message.write(&mut buffer).expect("writes");
        assert_eq!(buffer, b"username=x-access-token\npassword=s3cret\n\n");

        let echoed = Message::read(&mut buffer.as_slice()).expect("parses");
        assert_eq!(echoed, message);
    }

    #[test]
    fn a_newline_in_a_value_is_refused_not_escaped() {
        let mut message = Message::default();
        let error = message
            .push("password", "token\nhost=evil.example")
            .expect_err("refused");
        assert!(format!("{error}").contains("line break"), "{error}");
    }

    #[test]
    fn a_refusal_carries_only_the_error_key() {
        let message = Message::refusal("no credential for host=example.com").expect("valid");
        assert_eq!(
            message.get(ERROR_KEY),
            Some("no credential for host=example.com")
        );
        assert_eq!(message.get("password"), None);
    }
}
