//! `gcal`: Google Calendar from the shell.
//!
//! A thin surface over the [`google_calendar`] crate per RFC 0003: this file
//! shapes arguments and renders output, and the crate owns the API client,
//! OAuth, and error mapping. `--json` emits the crate's wire types verbatim,
//! which is also the contract the ix-mcp calendar tools consume.

use anyhow::{Context as _, bail, ensure};
use chrono::{
    DateTime, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta,
    TimeZone as _,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use google_calendar::{
    ALL_KNOWN_SCOPES, Attendee, AttendeeDraft, Authenticator, Client, ClientSecrets, EVENTS_SCOPE,
    Event, EventDraft, EventQuery, EventTime, PRIMARY_CALENDAR, SendUpdates, TokenStore,
    begin_consent,
};

/// Command-line arguments.
#[derive(Parser)]
#[command(name = "gcal", about = "Google Calendar from the shell", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authorize against your Google account and store the refresh token.
    ///
    /// Needs the team OAuth client in `GOOGLE_OAUTH_CLIENT_ID` and
    /// `GOOGLE_OAUTH_CLIENT_SECRET`. Prints a consent URL; with a local browser
    /// the redirect lands automatically, over SSH pass `--paste` and feed the
    /// redirect URL back on stdin.
    Auth(AuthArgs),
    /// Print a current OAuth access token minted from the stored grant.
    ///
    /// With `--json`, emits `{access_token, expires_in, scopes}`; this is
    /// the contract the bundled Python `google_auth` helper consumes to
    /// drive the Gmail and Calendar APIs. Without it, prints the bare
    /// token.
    PrintAccessToken(PrintAccessTokenArgs),
    /// List events in a window (default: now through 7 days from now).
    List(ListArgs),
    /// Show one event.
    Show(ShowArgs),
    /// Create an event.
    Create(CreateArgs),
    /// Cancel (delete) an event.
    Cancel(CancelArgs),
}

#[derive(Args)]
struct PrintAccessTokenArgs {
    /// Emit `{access_token, expires_in, scopes}` as JSON instead of the
    /// bare token (what the Python `google_auth` helper reads).
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct AuthArgs {
    /// Read the redirect URL from stdin instead of waiting on the loopback
    /// listener. Use this over SSH or in a VM, where the browser cannot reach
    /// this machine's `127.0.0.1`.
    #[arg(long)]
    paste: bool,
}

/// Which calendar to operate on.
#[derive(Args)]
struct CalendarArg {
    /// Calendar id: an email address, or `primary` for your own calendar.
    #[arg(long, default_value = PRIMARY_CALENDAR)]
    calendar: String,
}

#[derive(Args)]
struct ListArgs {
    #[command(flatten)]
    calendar: CalendarArg,

    /// Window start: RFC 3339, `YYYY-MM-DD HH:MM` (local), or `YYYY-MM-DD`.
    #[arg(long)]
    from: Option<String>,

    /// Window end, same formats as --from.
    #[arg(long)]
    to: Option<String>,

    /// Maximum number of events.
    #[arg(long, default_value_t = 20)]
    max: usize,

    /// Free-text filter (matches summary, description, attendees, ...).
    #[arg(long)]
    query: Option<String>,

    /// Emit the events as a JSON array instead of the human listing.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ShowArgs {
    /// The event id (shown in brackets by `gcal list`).
    event_id: String,

    #[command(flatten)]
    calendar: CalendarArg,

    /// Emit the event as JSON instead of the human block.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct CreateArgs {
    /// Event title.
    #[arg(long)]
    summary: String,

    /// Start: RFC 3339 or local `YYYY-MM-DD HH:MM`; with --all-day, a date.
    #[arg(long)]
    start: String,

    /// End instant; with --all-day, the last day (inclusive). Defaults to a
    /// single day for all-day events, otherwise required.
    #[arg(long)]
    end: Option<String>,

    /// Create an all-day event (--start/--end are dates).
    #[arg(long)]
    all_day: bool,

    /// Free-text body.
    #[arg(long)]
    description: Option<String>,

    /// Free-text location.
    #[arg(long)]
    location: Option<String>,

    /// Attendee email to invite (repeatable).
    #[arg(long = "attendee", value_name = "EMAIL")]
    attendees: Vec<String>,

    /// Who Google emails about the invite (mirrors the Calendar UI default).
    #[arg(long, value_enum, default_value_t = Notify::All)]
    notify: Notify,

    #[command(flatten)]
    calendar: CalendarArg,

    /// Emit the created event as JSON instead of the confirmation line.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct CancelArgs {
    /// The event id (shown in brackets by `gcal list`).
    event_id: String,

    #[command(flatten)]
    calendar: CalendarArg,

    /// Who Google emails about the cancellation.
    #[arg(long, value_enum, default_value_t = Notify::All)]
    notify: Notify,

    /// Emit a JSON confirmation instead of the human line.
    #[arg(long)]
    json: bool,
}

/// CLI spelling of [`SendUpdates`].
#[derive(Clone, Copy, ValueEnum)]
enum Notify {
    /// Notify every attendee.
    All,
    /// Notify only attendees outside the organizer's domain.
    ExternalOnly,
    /// Send no notifications.
    None,
}

impl From<Notify> for SendUpdates {
    fn from(notify: Notify) -> Self {
        match notify {
            Notify::All => Self::All,
            Notify::ExternalOnly => Self::ExternalOnly,
            Notify::None => Self::None,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Auth(args) => run_auth(args).await,
        Command::PrintAccessToken(args) => run_print_access_token(args).await,
        Command::List(args) => run_list(args).await,
        Command::Show(args) => run_show(args).await,
        Command::Create(args) => run_create(args).await,
        Command::Cancel(args) => run_cancel(args).await,
    }
}

/// Mint a current access token from the stored grant and print it. The
/// Authenticator is built with no required scopes: print-access-token
/// hands the token to a downstream caller that decides which scope set
/// it needs, and rejecting here would be a false negative.
async fn run_print_access_token(args: PrintAccessTokenArgs) -> anyhow::Result<()> {
    let auth = Authenticator::new(ClientSecrets::from_env()?, TokenStore::new()?, &[])?;
    let minted = auth.mint_access_token().await?;
    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "access_token": minted.token,
                "expires_in": minted.expires_in,
                "scopes": minted.scopes,
            })
        );
    } else {
        println!("{}", minted.token);
    }
    Ok(())
}

/// A client over the env credentials and the default token store.
fn client() -> anyhow::Result<Client> {
    let auth = Authenticator::new(
        ClientSecrets::from_env()?,
        TokenStore::new()?,
        &[EVENTS_SCOPE],
    )?;
    Ok(Client::new(auth)?)
}

async fn run_auth(args: AuthArgs) -> anyhow::Result<()> {
    let secrets = ClientSecrets::from_env()?;
    let store = TokenStore::new()?;
    // Consent to every scope the repo knows about so one consent flow
    // covers calendar + gmail; the per-binary scope check at runtime is
    // what enforces least privilege.
    let pending = begin_consent(secrets.clone(), ALL_KNOWN_SCOPES).await?;

    println!("Open this URL in your browser:\n\n  {}\n", pending.auth_url);
    let code = if args.paste {
        println!("After consenting, the browser shows a connection error on the");
        println!("http://127.0.0.1:… redirect; paste that full URL here and press enter.");
        let pasted = read_stdin_line()
            .await
            .context("reading the pasted redirect URL from stdin")?;
        pending.code_from_redirect_url(pasted.trim())?
    } else {
        println!("Waiting for the redirect on this machine's loopback listener.");
        println!("Over SSH or in a VM, cancel and rerun with --paste.");
        pending.wait_loopback().await?
    };

    let token = pending.exchange(code).await?;
    store.save(&token)?;
    println!("Token saved to {}", store.path().display());

    // Prove the grant end to end with the cheapest real read, so a scope or
    // clock problem surfaces now rather than on the first scripted call.
    let client = Client::new(Authenticator::new(secrets, store, &[EVENTS_SCOPE])?)?;
    let probe = EventQuery {
        time_min: Some(Local::now().fixed_offset()),
        time_max: None,
        text: None,
        max_events: 1,
    };
    client.list_events(PRIMARY_CALENDAR, &probe).await?;
    println!("Verified: the Calendar API answers with this grant.");
    Ok(())
}

async fn read_stdin_line() -> std::io::Result<String> {
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    let mut line = String::new();
    BufReader::new(tokio::io::stdin())
        .read_line(&mut line)
        .await?;
    Ok(line)
}

async fn run_list(args: ListArgs) -> anyhow::Result<()> {
    let from = match &args.from {
        Some(input) => parse_instant(input)?,
        None => Local::now().fixed_offset(),
    };
    let to = match &args.to {
        Some(input) => parse_instant(input)?,
        None => from + TimeDelta::days(7),
    };

    let query = EventQuery {
        time_min: Some(from),
        time_max: Some(to),
        text: args.query,
        max_events: args.max,
    };
    let events = client()?
        .list_events(&args.calendar.calendar, &query)
        .await?;

    if args.json {
        println!("{}", serde_json::to_string(&events)?);
    } else if events.is_empty() {
        println!(
            "no events from {} to {}",
            from.format("%Y-%m-%d %H:%M"),
            to.format("%Y-%m-%d %H:%M")
        );
    } else {
        for event in &events {
            println!("{}", event_line(event));
        }
    }
    Ok(())
}

async fn run_show(args: ShowArgs) -> anyhow::Result<()> {
    let event = client()?
        .get_event(&args.calendar.calendar, &args.event_id)
        .await?;
    if args.json {
        println!("{}", serde_json::to_string(&event)?);
    } else {
        println!("{}", event_block(&event));
    }
    Ok(())
}

async fn run_create(args: CreateArgs) -> anyhow::Result<()> {
    let (start, end) = if args.all_day {
        let window = all_day_window(&args.start, args.end.as_deref())?;
        (EventTime::AllDay { date: window.first }, window.end)
    } else {
        let start = parse_instant(&args.start)?;
        let end_input = args
            .end
            .as_deref()
            .context("--end is required for a timed event (or pass --all-day)")?;
        let end = parse_instant(end_input)?;
        ensure!(end > start, "--end must be after --start");
        (
            EventTime::Timed {
                date_time: start,
                time_zone: None,
            },
            EventTime::Timed {
                date_time: end,
                time_zone: None,
            },
        )
    };

    let draft = EventDraft {
        summary: args.summary,
        description: args.description,
        location: args.location,
        start,
        end,
        attendees: args
            .attendees
            .into_iter()
            .map(|email| AttendeeDraft { email })
            .collect(),
    };
    let created = client()?
        .create_event(&args.calendar.calendar, &draft, args.notify.into())
        .await?;

    if args.json {
        println!("{}", serde_json::to_string(&created)?);
    } else {
        println!("created {}", created.id);
        if let Some(link) = &created.html_link {
            println!("{link}");
        }
    }
    Ok(())
}

async fn run_cancel(args: CancelArgs) -> anyhow::Result<()> {
    client()?
        .cancel_event(&args.calendar.calendar, &args.event_id, args.notify.into())
        .await?;
    if args.json {
        println!("{}", serde_json::json!({ "cancelled": args.event_id }));
    } else {
        println!("cancelled {}", args.event_id);
    }
    Ok(())
}

/// An all-day span in the API's shape: first day, plus the exclusive end.
#[derive(Debug)]
struct AllDayWindow {
    first: NaiveDate,
    /// Google's exclusive `end.date`: the day after the last day.
    end: EventTime,
}

/// Resolve `--start`/`--end` dates for `--all-day`. The CLI takes the last
/// day inclusive (how humans say "June 10 to June 12"); the crate owns the
/// conversion to the API's exclusive end date.
fn all_day_window(start: &str, end: Option<&str>) -> anyhow::Result<AllDayWindow> {
    let first = parse_date(start)?;
    let last = match end {
        Some(input) => {
            let last = parse_date(input)?;
            ensure!(
                last >= first,
                "--end (the last day) must not be before --start"
            );
            last
        }
        None => first,
    };
    let end = EventTime::all_day_end_from_inclusive(last)
        .context("--end is out of the representable date range")?;
    Ok(AllDayWindow { first, end })
}

/// Parse a point in time: RFC 3339 with offset, a naive local datetime, or a
/// date (midnight local).
fn parse_instant(input: &str) -> anyhow::Result<DateTime<FixedOffset>> {
    if let Ok(instant) = DateTime::parse_from_rfc3339(input) {
        return Ok(instant);
    }
    let naive = parse_naive(input)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(local) => Ok(local.fixed_offset()),
        // A DST transition makes this wall-clock time ambiguous or
        // nonexistent; guessing a side silently shifts the event by an hour.
        _ => bail!(
            "{input:?} is ambiguous or skipped in this timezone (DST); \
             pass an RFC 3339 time with offset like 2026-06-05T09:30:00-07:00"
        ),
    }
}

fn parse_naive(input: &str) -> anyhow::Result<NaiveDateTime> {
    const FORMATS: [&str; 4] = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ];
    for format in FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(input, format) {
            return Ok(naive);
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        return Ok(date.and_time(NaiveTime::MIN));
    }
    bail!(
        "could not parse {input:?} as a time; use RFC 3339 (2026-06-05T09:30:00-07:00), \
         a local time (2026-06-05 09:30), or a date (2026-06-05)"
    )
}

fn parse_date(input: &str) -> anyhow::Result<NaiveDate> {
    NaiveDate::parse_from_str(input, "%Y-%m-%d")
        .with_context(|| format!("could not parse {input:?} as a date (YYYY-MM-DD)"))
}

/// One listing line: when, title, id.
fn event_line(event: &Event) -> String {
    let when = event.start.as_ref().map_or_else(
        || "(no time)".to_owned(),
        |start| span(start, event.end.as_ref()),
    );
    let summary = event.summary.as_deref().unwrap_or("(no title)");
    format!("{when}  {summary}  [{}]", event.id)
}

/// Render a start/end pair compactly, in the offset the API returned (the
/// calendar's own timezone).
fn span(start: &EventTime, end: Option<&EventTime>) -> String {
    match (start, end) {
        (EventTime::AllDay { date }, Some(EventTime::AllDay { date: end_date })) => {
            // `end.date` is exclusive; show the human last day.
            match end_date.pred_opt() {
                Some(last) if last > *date => format!("{date} – {last} (all day)"),
                _ => format!("{date} (all day)"),
            }
        }
        (EventTime::AllDay { date }, _) => format!("{date} (all day)"),
        (
            EventTime::Timed { date_time, .. },
            Some(EventTime::Timed {
                date_time: end_time,
                ..
            }),
        ) => {
            if date_time.date_naive() == end_time.date_naive() {
                format!(
                    "{} {}–{}",
                    date_time.format("%Y-%m-%d"),
                    date_time.format("%H:%M"),
                    end_time.format("%H:%M"),
                )
            } else {
                format!(
                    "{} – {}",
                    date_time.format("%Y-%m-%d %H:%M"),
                    end_time.format("%Y-%m-%d %H:%M"),
                )
            }
        }
        (EventTime::Timed { date_time, .. }, _) => date_time.format("%Y-%m-%d %H:%M").to_string(),
    }
}

/// The `gcal show` block: title, then aligned fields, then the description.
fn event_block(event: &Event) -> String {
    let mut lines = vec![
        event
            .summary
            .clone()
            .unwrap_or_else(|| "(no title)".to_owned()),
    ];
    if let Some(start) = &event.start {
        lines.push(format!("  when:      {}", span(start, event.end.as_ref())));
    }
    if let Some(status) = &event.status {
        lines.push(format!("  status:    {status}"));
    }
    if let Some(location) = &event.location {
        lines.push(format!("  location:  {location}"));
    }
    if let Some(email) = event
        .organizer
        .as_ref()
        .and_then(|organizer| organizer.email.as_ref())
    {
        lines.push(format!("  organizer: {email}"));
    }
    if !event.attendees.is_empty() {
        let attendees: Vec<String> = event.attendees.iter().map(attendee_label).collect();
        lines.push(format!("  attendees: {}", attendees.join(", ")));
    }
    if let Some(meet) = &event.hangout_link {
        lines.push(format!("  meet:      {meet}"));
    }
    if let Some(link) = &event.html_link {
        lines.push(format!("  link:      {link}"));
    }
    if let Some(description) = &event.description {
        lines.push(String::new());
        lines.push(description.trim_end().to_owned());
    }
    lines.join("\n")
}

fn attendee_label(attendee: &Attendee) -> String {
    let email = attendee.email.as_deref().unwrap_or("(no email)");
    attendee
        .response_status
        .as_deref()
        .map_or_else(|| email.to_owned(), |status| format!("{email} ({status})"))
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use google_calendar::{Event, EventTime};

    use super::{all_day_window, event_line, parse_instant, span};

    fn date(s: &str) -> NaiveDate {
        s.parse().expect("test date")
    }

    #[test]
    fn rfc3339_input_keeps_its_offset() {
        let instant = parse_instant("2026-06-05T09:30:00-07:00").expect("parses");
        assert_eq!(instant.to_rfc3339(), "2026-06-05T09:30:00-07:00");
    }

    #[test]
    fn naive_input_resolves_in_local_time() {
        let instant = parse_instant("2026-06-05 09:30").expect("parses");
        assert_eq!(instant.naive_local().to_string(), "2026-06-05 09:30:00");
    }

    #[test]
    fn date_input_means_local_midnight() {
        let instant = parse_instant("2026-06-05").expect("parses");
        assert_eq!(instant.naive_local().to_string(), "2026-06-05 00:00:00");
    }

    #[test]
    fn garbage_time_input_names_the_accepted_formats() {
        let err = parse_instant("tomorrow-ish").expect_err("rejects");
        assert!(err.to_string().contains("RFC 3339"), "got: {err}");
    }

    #[test]
    fn all_day_end_is_inclusive_at_the_cli_and_exclusive_on_the_wire() {
        let window = all_day_window("2026-06-10", Some("2026-06-12")).expect("window");
        assert_eq!(window.first, date("2026-06-10"));
        assert_eq!(
            window.end,
            EventTime::AllDay {
                date: date("2026-06-13"),
            }
        );
    }

    #[test]
    fn all_day_defaults_to_one_day() {
        let window = all_day_window("2026-06-10", None).expect("window");
        assert_eq!(
            window.end,
            EventTime::AllDay {
                date: date("2026-06-11"),
            }
        );
    }

    #[test]
    fn all_day_end_before_start_is_rejected() {
        let err = all_day_window("2026-06-10", Some("2026-06-09")).expect_err("rejects");
        assert!(err.to_string().contains("--end"), "got: {err}");
    }

    #[test]
    fn same_day_span_collapses_the_date() {
        let start = EventTime::Timed {
            date_time: "2026-06-05T09:30:00-07:00".parse().expect("start"),
            time_zone: None,
        };
        let end = EventTime::Timed {
            date_time: "2026-06-05T10:00:00-07:00".parse().expect("end"),
            time_zone: None,
        };
        assert_eq!(span(&start, Some(&end)), "2026-06-05 09:30–10:00");
    }

    #[test]
    fn multi_day_all_day_span_shows_the_inclusive_last_day() {
        let start = EventTime::AllDay {
            date: date("2026-06-10"),
        };
        let end = EventTime::AllDay {
            date: date("2026-06-13"),
        };
        assert_eq!(
            span(&start, Some(&end)),
            "2026-06-10 – 2026-06-12 (all day)"
        );
    }

    #[test]
    fn cancelled_stub_still_renders_a_line() {
        let event: Event =
            serde_json::from_str(r#"{"id":"abc","status":"cancelled"}"#).expect("stub parses");
        assert_eq!(event_line(&event), "(no time)  (no title)  [abc]");
    }
}
