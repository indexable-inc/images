//! Parse `--start`/`--end` boundary strings into wire [`EventTime`]s.
//!
//! The Rust MCP server (`packages/google/mcp`) and the `PyO3` bindings
//! (`packages/google/py`) both take an event boundary as a string plus an
//! `all_day` flag, and both must agree on how it parses and on the
//! inclusive-to-exclusive all-day end conversion. One parser here keeps the
//! tool surface and the Python surface from drifting (RFC 0003); each
//! surface maps [`ParseEventTimeError`] onto its own error type.

use chrono::{DateTime, NaiveDate};
use snafu::{OptionExt as _, Snafu};

use crate::model::EventTime;

/// Failure parsing an event start or end boundary.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum ParseEventTimeError {
    /// The boundary string did not match the shape required for its kind: a
    /// `YYYY-MM-DD` date for an all-day boundary, an RFC 3339 instant for a
    /// timed one.
    ///
    /// `reason` is a plain field rather than a snafu `source` because
    /// `chrono::ParseError` only implements `std::error::Error` under
    /// chrono's `std` feature, which this crate deliberately leaves off
    /// (default-features = false, `alloc` only).
    #[snafu(display("{field}: could not parse {input:?} as {expected}: {reason}"))]
    Parse {
        /// Which boundary (`start` or `end`).
        field: &'static str,
        /// The rejected input.
        input: String,
        /// The shape the parser expected (`YYYY-MM-DD` or `RFC 3339`).
        expected: &'static str,
        /// Underlying parse failure.
        reason: chrono::ParseError,
    },

    /// The inclusive last day is the last representable date, so there is no
    /// day after it to use as Google's exclusive all-day `end.date`.
    #[snafu(display("end: no day follows {last}"))]
    NoDayFollows {
        /// The inclusive last day with no successor.
        last: NaiveDate,
    },
}

/// The `map_err` for a boundary parse: names the field, the rejected input,
/// and the shape that was expected. One constructor keeps the all-day and
/// timed branches from each spelling out the same closure.
fn parse_failure(
    field: &'static str,
    input: &str,
    expected: &'static str,
) -> impl FnOnce(chrono::ParseError) -> ParseEventTimeError {
    let input = input.to_owned();
    move |reason| {
        ParseSnafu {
            field,
            input,
            expected,
            reason,
        }
        .build()
    }
}

/// Parse one event boundary: an all-day `YYYY-MM-DD` date when `all_day`,
/// otherwise an RFC 3339 instant with offset. `field` names the boundary
/// (`start` or `end`) for the error message.
///
/// # Errors
/// Returns [`ParseEventTimeError`] if the input does not match the expected
/// shape for `all_day`.
pub fn parse_event_time(
    input: &str,
    all_day: bool,
    field: &'static str,
) -> Result<EventTime, ParseEventTimeError> {
    if all_day {
        let date = input
            .parse()
            .map_err(parse_failure(field, input, "YYYY-MM-DD"))?;
        Ok(EventTime::AllDay { date })
    } else {
        let date_time =
            DateTime::parse_from_rfc3339(input).map_err(parse_failure(field, input, "RFC 3339"))?;
        Ok(EventTime::Timed {
            date_time,
            time_zone: None,
        })
    }
}

/// Parse the `end` of an event. All-day input is the inclusive last day (how
/// the CLI, tools, and bindings document it); Google's all-day `end.date` is
/// exclusive, so convert at this boundary.
///
/// # Errors
/// Returns [`ParseEventTimeError`] if the input does not parse, or if an
/// all-day last day has no representable successor.
pub fn parse_event_end(input: &str, all_day: bool) -> Result<EventTime, ParseEventTimeError> {
    match parse_event_time(input, all_day, "end")? {
        EventTime::AllDay { date } => {
            EventTime::all_day_end_from_inclusive(date).context(NoDayFollowsSnafu { last: date })
        }
        timed @ EventTime::Timed { .. } => Ok(timed),
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::{ParseEventTimeError, parse_event_end, parse_event_time};
    use crate::model::EventTime;

    #[test]
    fn all_day_end_is_inclusive_at_the_surface_and_exclusive_on_the_wire() {
        let end = parse_event_end("2026-06-12", true).expect("parses");
        assert_eq!(
            end,
            EventTime::AllDay {
                date: "2026-06-13".parse().expect("date"),
            }
        );
    }

    #[test]
    fn timed_end_passes_through_unchanged() {
        let end = parse_event_end("2026-06-05T10:00:00-07:00", false).expect("parses");
        assert_eq!(
            end,
            EventTime::Timed {
                date_time: "2026-06-05T10:00:00-07:00".parse().expect("instant"),
                time_zone: None,
            }
        );
    }

    #[test]
    fn all_day_start_parses_the_bare_date() {
        let start = parse_event_time("2026-06-05", true, "start").expect("parses");
        assert_eq!(
            start,
            EventTime::AllDay {
                date: "2026-06-05".parse().expect("date"),
            }
        );
    }

    #[test]
    fn bad_all_day_date_names_the_field_and_the_expected_shape() {
        let err = parse_event_time("not-a-date", true, "start").expect_err("rejects");
        assert!(
            matches!(err, ParseEventTimeError::Parse { .. }),
            "got {err:?}"
        );
        let message = err.to_string();
        assert!(message.contains("start"), "names the field: {message}");
        assert!(message.contains("YYYY-MM-DD"), "names the shape: {message}");
    }

    #[test]
    fn bad_timed_input_names_rfc_3339() {
        let err = parse_event_time("2026-06-05 09:30", false, "end").expect_err("rejects");
        assert!(
            matches!(err, ParseEventTimeError::Parse { .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("RFC 3339"), "got: {err}");
    }

    #[test]
    fn no_day_follows_the_last_representable_date() {
        let last = NaiveDate::MAX;
        let err = parse_event_end(&last.to_string(), true).expect_err("no successor");
        assert!(
            matches!(err, ParseEventTimeError::NoDayFollows { .. }),
            "got {err:?}"
        );
    }
}
