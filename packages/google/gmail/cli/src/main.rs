//! `gmail`: Gmail from the shell.
//!
//! A thin surface over the [`google_gmail`] crate per RFC 0003: this file
//! shapes arguments and renders output, and the crate owns the API client,
//! OAuth, and error mapping. `--json` emits the crate's wire types
//! verbatim, which is also the contract the ix-google-mcp tools consume.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::{Args, Parser, Subcommand};
use google_gmail::{
    ALL_KNOWN_SCOPES, Attachment, Authenticator, Client, ClientSecrets, GMAIL_MODIFY, GMAIL_SEND,
    Message, MessageFormat, MessagePart, MessageQuery, OutgoingMessage, TokenStore, begin_consent,
};

#[derive(Parser)]
#[command(name = "gmail", about = "Gmail from the shell", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authorize against your Google account and store the refresh token.
    ///
    /// Needs the team OAuth client in `GOOGLE_OAUTH_CLIENT_ID` and
    /// `GOOGLE_OAUTH_CLIENT_SECRET`. Prints a consent URL; with a local
    /// browser the redirect lands automatically, over SSH pass `--paste`
    /// and feed the redirect URL back on stdin.
    Auth(AuthArgs),
    /// Sign out: delete this machine's stored Google grant.
    ///
    /// Removes the local token file (and any legacy one); the next call needs
    /// a fresh `gmail auth`. This does not revoke the grant at Google -- do
    /// that at <https://myaccount.google.com/permissions>.
    Logout(LogoutArgs),
    /// List messages, most recent first.
    List(ListArgs),
    /// Show one message (headers + body).
    Show(ShowArgs),
    /// Search messages with the Gmail query syntax (alias for `list -q`).
    Search(SearchArgs),
    /// Compose and send a message.
    Send(SendArgs),
    /// Manage drafts.
    Draft {
        #[command(subcommand)]
        command: DraftCommand,
    },
    /// Show one thread (all messages in order).
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    /// Manage labels on a message.
    Label {
        #[command(subcommand)]
        command: LabelCommand,
    },
    /// Manage attachments.
    Attach {
        #[command(subcommand)]
        command: AttachCommand,
    },
    /// Archive a message (remove the INBOX label).
    Archive(SingleIdArgs),
    /// Move a message to Trash.
    Trash(SingleIdArgs),
    /// Restore a message from Trash.
    Untrash(SingleIdArgs),
    /// Remove the UNREAD label.
    MarkRead(SingleIdArgs),
    /// Add the UNREAD label.
    MarkUnread(SingleIdArgs),
}

#[derive(Args)]
struct AuthArgs {
    /// Read the redirect URL from stdin instead of waiting on the loopback
    /// listener. Use this over SSH or in a VM, where the browser cannot
    /// reach this machine's `127.0.0.1`.
    #[arg(long)]
    paste: bool,

    /// Drive the flow as newline-delimited JSON instead of prose: first line
    /// `{"auth_url": "..."}` (flushed before the redirect wait so a caller can
    /// open a browser), then `{"signed_in": true, "scopes": [...]}` once the
    /// grant is stored. This is the contract the bundled Python
    /// `google_auth.login()` helper drives.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct LogoutArgs {
    /// Emit a JSON confirmation (`{"signed_out", "removed": [...]}`) instead
    /// of the human lines.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ListArgs {
    /// Gmail search syntax (`from:`, `to:`, `newer_than:`, `label:`, ...).
    #[arg(long, short = 'q')]
    query: Option<String>,

    /// Restrict to messages carrying every label in this set (repeatable).
    #[arg(long = "label", value_name = "LABEL_ID")]
    labels: Vec<String>,

    /// Include spam and trash. Off by default.
    #[arg(long)]
    include_spam_trash: bool,

    /// Maximum number of messages.
    #[arg(long, default_value_t = 20)]
    max: usize,

    /// Emit the messages as a JSON array of metadata projections.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ShowArgs {
    /// The message id.
    message_id: String,
    /// Fetch only headers, not the body.
    #[arg(long)]
    metadata: bool,
    /// Emit the message as JSON instead of the human block.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct SearchArgs {
    /// Gmail search query.
    query: String,
    /// Group by thread instead of by message.
    #[arg(long)]
    threads: bool,
    /// Maximum number of results.
    #[arg(long, default_value_t = 20)]
    max: usize,
    /// Emit as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct SendArgs {
    #[command(flatten)]
    compose: ComposeArgs,
    /// Emit the sent message as JSON instead of the confirmation.
    #[arg(long)]
    json: bool,
}

/// Compose-time fields shared by `send` and `draft create`/`draft update`.
#[derive(Args, Clone)]
struct ComposeArgs {
    /// Recipient (repeatable; To header).
    #[arg(long = "to", value_name = "EMAIL", required = true)]
    to: Vec<String>,
    /// Carbon-copy recipient (repeatable).
    #[arg(long = "cc", value_name = "EMAIL")]
    cc: Vec<String>,
    /// Blind-carbon-copy recipient (repeatable).
    #[arg(long = "bcc", value_name = "EMAIL")]
    bcc: Vec<String>,
    /// Subject line.
    #[arg(long)]
    subject: String,
    /// Path to the plain-text body (`-` for stdin).
    #[arg(long, value_name = "FILE")]
    body: Option<PathBuf>,
    /// Path to the HTML body (`-` for stdin).
    #[arg(long, value_name = "FILE")]
    html: Option<PathBuf>,
    /// Attachment path (repeatable).
    #[arg(long = "attach", value_name = "FILE")]
    attachments: Vec<PathBuf>,
    /// Thread to attach the message to (reply).
    #[arg(long = "thread", value_name = "THREAD_ID")]
    thread_id: Option<String>,
}

#[derive(Subcommand)]
enum DraftCommand {
    /// Save a new draft.
    Create(DraftCreateArgs),
    /// Replace an existing draft.
    Update(DraftUpdateArgs),
    /// Send an existing draft.
    Send(DraftSendArgs),
    /// List drafts (id, snippet).
    List(DraftListArgs),
    /// Delete a draft.
    Delete(SingleIdArgs),
    /// Read a draft (headers + body).
    Show(SingleIdArgs),
}

#[derive(Args)]
struct DraftCreateArgs {
    #[command(flatten)]
    compose: ComposeArgs,
    /// Emit the saved draft as JSON instead of the id.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DraftUpdateArgs {
    /// The draft id to replace.
    draft_id: String,
    #[command(flatten)]
    compose: ComposeArgs,
    /// Emit the updated draft as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DraftSendArgs {
    /// The draft id to send.
    draft_id: String,
    /// Emit the sent message as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DraftListArgs {
    /// Maximum number of drafts.
    #[arg(long, default_value_t = 20)]
    max: usize,
    /// Emit as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum ThreadCommand {
    /// Show one thread.
    Show(ThreadShowArgs),
}

#[derive(Args)]
struct ThreadShowArgs {
    /// The thread id.
    thread_id: String,
    /// Fetch only headers, not bodies.
    #[arg(long)]
    metadata: bool,
    /// Emit as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum LabelCommand {
    /// List labels.
    List(LabelListArgs),
    /// Apply a label to a message.
    Apply(LabelMutateArgs),
    /// Remove a label from a message.
    Remove(LabelMutateArgs),
}

#[derive(Args)]
struct LabelListArgs {
    /// Emit as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct LabelMutateArgs {
    /// The message id.
    message_id: String,
    /// The label id (e.g. `INBOX`, `STARRED`, or a `Label_*` user id).
    label_id: String,
}

#[derive(Subcommand)]
enum AttachCommand {
    /// Download an attachment to disk or stdout.
    Get(AttachGetArgs),
}

#[derive(Args)]
struct AttachGetArgs {
    /// The message id.
    message_id: String,
    /// The attachment id (shown in `gmail show --json` under the body's
    /// `attachmentId`).
    attachment_id: String,
    /// Write to this path. Defaults to stdout.
    #[arg(short = 'o', long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct SingleIdArgs {
    /// The message id.
    message_id: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Auth(args) => run_auth(args).await,
        Command::Logout(args) => run_logout(args.json),
        Command::List(args) => run_list(args).await,
        Command::Show(args) => run_show(args).await,
        Command::Search(args) => run_search(args).await,
        Command::Send(args) => run_send(args).await,
        Command::Draft { command } => run_draft(command).await,
        Command::Thread { command } => run_thread(command).await,
        Command::Label { command } => run_label(command).await,
        Command::Attach { command } => run_attach(command).await,
        Command::Archive(args) => run_simple_modify(args, Action::Archive).await,
        Command::Trash(args) => run_simple_modify(args, Action::Trash).await,
        Command::Untrash(args) => run_simple_modify(args, Action::Untrash).await,
        Command::MarkRead(args) => run_simple_modify(args, Action::MarkRead).await,
        Command::MarkUnread(args) => run_simple_modify(args, Action::MarkUnread).await,
    }
}

/// A client over the env credentials and the default token store.
fn client() -> anyhow::Result<Client> {
    let auth = Authenticator::new(
        ClientSecrets::from_env()?,
        TokenStore::new()?,
        &[GMAIL_MODIFY, GMAIL_SEND],
    )?;
    Ok(Client::new(auth)?)
}

async fn run_auth(args: AuthArgs) -> anyhow::Result<()> {
    use std::io::Write as _;

    let secrets = ClientSecrets::from_env()?;
    let store = TokenStore::new()?;
    // Consent to every scope the repo knows about so one consent flow
    // covers calendar + gmail; the per-binary scope check at runtime is
    // what enforces least privilege.
    let pending = begin_consent(secrets.clone(), ALL_KNOWN_SCOPES).await?;

    if args.json {
        // NDJSON line 1: the consent URL. Flush it before blocking on the
        // redirect so a driver (the Python `login()` helper) can open a
        // browser while this process waits.
        println!("{}", serde_json::json!({ "auth_url": pending.auth_url }));
        std::io::stdout().flush().context("flushing the consent URL")?;
    } else {
        println!("Open this URL in your browser:\n\n  {}\n", pending.auth_url);
    }

    let code = if args.paste {
        if !args.json {
            println!("After consenting, the browser shows a connection error on the");
            println!("http://127.0.0.1:… redirect; paste that full URL here and press enter.");
        }
        let pasted = read_stdin_line()
            .await
            .context("reading the pasted redirect URL from stdin")?;
        pending.code_from_redirect_url(pasted.trim())?
    } else {
        if !args.json {
            println!("Waiting for the redirect on this machine's loopback listener.");
            println!("Over SSH or in a VM, cancel and rerun with --paste.");
        }
        pending.wait_loopback().await?
    };

    let token = pending.exchange(code).await?;
    store.save(&token)?;

    // Prove the grant end to end with the cheapest real read, so a scope
    // or clock problem surfaces now rather than on the first scripted call.
    let client = Client::new(Authenticator::new(secrets, store.clone(), &[GMAIL_MODIFY])?)?;
    client.list_labels().await?;

    if args.json {
        // NDJSON line 2: the grant is stored and verified.
        println!(
            "{}",
            serde_json::json!({
                "signed_in": true,
                "scopes": token.scopes,
                "token_path": store.path().display().to_string(),
            })
        );
    } else {
        println!("Token saved to {}", store.path().display());
        println!("Verified: the Gmail API answers with this grant.");
    }
    Ok(())
}

/// Delete the stored grant. Idempotent: signing out when already signed out
/// is a no-op, not an error.
fn run_logout(json: bool) -> anyhow::Result<()> {
    let removed = TokenStore::new()?.remove()?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "signed_out": !removed.is_empty(),
                "removed": removed
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>(),
            })
        );
    } else if removed.is_empty() {
        println!("Already signed out: no stored Google token.");
    } else {
        for path in &removed {
            println!("Removed {}", path.display());
        }
        println!(
            "Signed out. To fully revoke access, also remove it at \
             https://myaccount.google.com/permissions"
        );
    }
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

async fn read_body_source(path: &Path) -> anyhow::Result<String> {
    if path == Path::new("-") {
        use tokio::io::AsyncReadExt as _;
        let mut buf = String::new();
        tokio::io::stdin()
            .read_to_string(&mut buf)
            .await
            .context("reading body from stdin")?;
        return Ok(buf);
    }
    tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("reading body from {}", path.display()))
}

async fn run_list(args: ListArgs) -> anyhow::Result<()> {
    let client = client()?;
    let query = MessageQuery {
        q: args.query,
        label_ids: args.labels,
        include_spam_trash: args.include_spam_trash,
        max_results: args.max,
    };
    let stubs = client.list_messages(&query).await?;
    let messages = enrich_metadata(&client, stubs).await?;

    if args.json {
        println!("{}", serde_json::to_string(&messages)?);
    } else if messages.is_empty() {
        println!("no messages");
    } else {
        for message in &messages {
            println!("{}", message_line(message));
        }
    }
    Ok(())
}

async fn run_show(args: ShowArgs) -> anyhow::Result<()> {
    let client = client()?;
    let format = if args.metadata {
        MessageFormat::Metadata
    } else {
        MessageFormat::Full
    };
    let message = client.get_message(&args.message_id, format).await?;
    if args.json {
        println!("{}", serde_json::to_string(&message)?);
    } else {
        println!("{}", message_block(&message));
    }
    Ok(())
}

async fn run_search(args: SearchArgs) -> anyhow::Result<()> {
    let client = client()?;
    let query = MessageQuery {
        q: Some(args.query),
        max_results: args.max,
        ..MessageQuery::default()
    };
    if args.threads {
        let threads = client.list_threads(&query).await?;
        if args.json {
            println!("{}", serde_json::to_string(&threads)?);
        } else {
            for thread in &threads {
                let snippet = thread.snippet.as_deref().unwrap_or("");
                println!("{}  {}", thread.id, snippet);
            }
        }
    } else {
        run_list(ListArgs {
            query: query.q,
            labels: Vec::new(),
            include_spam_trash: false,
            max: args.max,
            json: args.json,
        })
        .await?;
    }
    Ok(())
}

async fn run_send(args: SendArgs) -> anyhow::Result<()> {
    let client = client()?;
    let message = compose_outgoing(args.compose).await?;
    let sent = client.send_message(&message).await?;
    if args.json {
        println!("{}", serde_json::to_string(&sent)?);
    } else {
        println!("sent {}", sent.id);
    }
    Ok(())
}

async fn run_draft(command: DraftCommand) -> anyhow::Result<()> {
    let client = client()?;
    match command {
        DraftCommand::Create(args) => {
            let message = compose_outgoing(args.compose).await?;
            let draft = client.create_draft(&message).await?;
            if args.json {
                println!("{}", serde_json::to_string(&draft)?);
            } else {
                println!("created {}", draft.id);
            }
        }
        DraftCommand::Update(args) => {
            let message = compose_outgoing(args.compose).await?;
            let draft = client.update_draft(&args.draft_id, &message).await?;
            if args.json {
                println!("{}", serde_json::to_string(&draft)?);
            } else {
                println!("updated {}", draft.id);
            }
        }
        DraftCommand::Send(args) => {
            let sent = client.send_draft(&args.draft_id).await?;
            if args.json {
                println!("{}", serde_json::to_string(&sent)?);
            } else {
                println!("sent {}", sent.id);
            }
        }
        DraftCommand::List(args) => {
            let drafts = client.list_drafts(args.max).await?;
            if args.json {
                println!("{}", serde_json::to_string(&drafts)?);
            } else {
                for draft in &drafts {
                    println!("{}  [thread {}]", draft.id, draft.message.thread_id);
                }
            }
        }
        DraftCommand::Delete(args) => {
            client.delete_draft(&args.message_id).await?;
            println!("deleted {}", args.message_id);
        }
        DraftCommand::Show(args) => {
            let draft = client.get_draft(&args.message_id).await?;
            println!("{}", message_block(&draft.message));
        }
    }
    Ok(())
}

async fn run_thread(command: ThreadCommand) -> anyhow::Result<()> {
    let client = client()?;
    match command {
        ThreadCommand::Show(args) => {
            let format = if args.metadata {
                MessageFormat::Metadata
            } else {
                MessageFormat::Full
            };
            let thread = client.get_thread(&args.thread_id, format).await?;
            if args.json {
                println!("{}", serde_json::to_string(&thread)?);
            } else {
                for message in &thread.messages {
                    println!("{}", message_block(message));
                    println!();
                }
            }
        }
    }
    Ok(())
}

async fn run_label(command: LabelCommand) -> anyhow::Result<()> {
    let client = client()?;
    match command {
        LabelCommand::List(args) => {
            let labels = client.list_labels().await?;
            if args.json {
                println!("{}", serde_json::to_string(&labels)?);
            } else {
                for label in &labels {
                    let kind = label.kind.as_deref().unwrap_or("?");
                    println!("{:<24}  ({})  {}", label.id, kind, label.name);
                }
            }
        }
        LabelCommand::Apply(args) => {
            let _ = client
                .modify_labels(&args.message_id, std::slice::from_ref(&args.label_id), &[])
                .await?;
            println!("applied {} to {}", args.label_id, args.message_id);
        }
        LabelCommand::Remove(args) => {
            let _ = client
                .modify_labels(&args.message_id, &[], std::slice::from_ref(&args.label_id))
                .await?;
            println!("removed {} from {}", args.label_id, args.message_id);
        }
    }
    Ok(())
}

async fn run_attach(command: AttachCommand) -> anyhow::Result<()> {
    let client = client()?;
    match command {
        AttachCommand::Get(args) => {
            let bytes = client
                .get_attachment(&args.message_id, &args.attachment_id)
                .await?;
            if let Some(path) = args.output {
                tokio::fs::write(&path, &bytes)
                    .await
                    .with_context(|| format!("writing attachment to {}", path.display()))?;
                println!("wrote {} bytes to {}", bytes.len(), path.display());
            } else {
                use tokio::io::AsyncWriteExt as _;
                let mut stdout = tokio::io::stdout();
                stdout
                    .write_all(&bytes)
                    .await
                    .context("writing to stdout")?;
                stdout.flush().await.ok();
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Action {
    Archive,
    Trash,
    Untrash,
    MarkRead,
    MarkUnread,
}

async fn run_simple_modify(args: SingleIdArgs, action: Action) -> anyhow::Result<()> {
    let client = client()?;
    match action {
        Action::Archive => {
            client.archive_message(&args.message_id).await?;
            println!("archived {}", args.message_id);
        }
        Action::Trash => {
            client.trash_message(&args.message_id).await?;
            println!("trashed {}", args.message_id);
        }
        Action::Untrash => {
            client.untrash_message(&args.message_id).await?;
            println!("untrashed {}", args.message_id);
        }
        Action::MarkRead => {
            client.mark_message_read(&args.message_id).await?;
            println!("marked read {}", args.message_id);
        }
        Action::MarkUnread => {
            client.mark_message_unread(&args.message_id).await?;
            println!("marked unread {}", args.message_id);
        }
    }
    Ok(())
}

async fn compose_outgoing(args: ComposeArgs) -> anyhow::Result<OutgoingMessage> {
    let body_text = match &args.body {
        Some(path) => Some(read_body_source(path).await?),
        None => None,
    };
    let body_html = match &args.html {
        Some(path) => Some(read_body_source(path).await?),
        None => None,
    };
    if body_text.is_none() && body_html.is_none() {
        bail!("at least one of --body or --html is required");
    }

    let mut attachments = Vec::with_capacity(args.attachments.len());
    for path in args.attachments {
        let content = tokio::fs::read(&path)
            .await
            .with_context(|| format!("reading attachment {}", path.display()))?;
        let filename = path
            .file_name()
            .and_then(|os| os.to_str())
            .with_context(|| format!("attachment path has no filename: {}", path.display()))?
            .to_owned();
        let content_type = guess_content_type(&filename);
        attachments.push(Attachment {
            filename,
            content_type,
            content,
        });
    }

    Ok(OutgoingMessage {
        to: args.to,
        cc: args.cc,
        bcc: args.bcc,
        subject: args.subject,
        body_text,
        body_html,
        thread_id: args.thread_id,
        attachments,
    })
}

/// Heuristic content-type from the filename extension. The MCP server and
/// the CLI go through this same path, so a wrong guess in one is a wrong
/// guess in both.
fn guess_content_type(filename: &str) -> String {
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|os| os.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "csv" => "text/csv",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_owned()
}

/// One listing line for a message: date, from, subject, [id].
fn message_line(message: &Message) -> String {
    let date = message.internal_date.map_or_else(
        || "                ".to_owned(),
        |dt| dt.format("%Y-%m-%d %H:%M").to_string(),
    );
    let from = message
        .payload
        .as_ref()
        .and_then(|payload| payload.header("From"))
        .unwrap_or("(unknown)");
    let subject = message
        .payload
        .as_ref()
        .and_then(|payload| payload.header("Subject"))
        .unwrap_or("(no subject)");
    format!("{date}  {from:<30.30}  {subject}  [{}]", message.id)
}

/// The `gmail show` block: aligned headers, then the decoded body.
fn message_block(message: &Message) -> String {
    let mut lines = Vec::new();
    if let Some(payload) = &message.payload {
        for name in ["From", "To", "Cc", "Date", "Subject"] {
            if let Some(value) = payload.header(name) {
                lines.push(format!("  {name:<8}: {value}"));
            }
        }
    }
    lines.push(format!("  labels:   {}", message.label_ids.join(" ")));
    if let Some(snippet) = &message.snippet {
        lines.push(format!("  snippet:  {snippet}"));
    }
    lines.push(format!("  id:       {}", message.id));
    if let Some(thread) = &message.thread_id {
        lines.push(format!("  thread:   {thread}"));
    }

    if let Some(body) = message.payload.as_ref().and_then(first_text_body) {
        lines.push(String::new());
        lines.push(body);
    }
    lines.join("\n")
}

/// Walk the part tree and return the first decoded text/plain (falling
/// back to text/html when no plain body exists).
fn first_text_body(payload: &MessagePart) -> Option<String> {
    fn walk(part: &MessagePart, want: &str) -> Option<String> {
        if part.mime_type.as_deref() == Some(want)
            && let Some(body) = &part.body
            && let Some(data) = &body.data
        {
            let bytes = URL_SAFE_NO_PAD.decode(data).ok()?;
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        for child in &part.parts {
            if let Some(found) = walk(child, want) {
                return Some(found);
            }
        }
        None
    }
    walk(payload, "text/plain").or_else(|| walk(payload, "text/html"))
}

/// Take the page of `MessageStub`s and fetch each one's metadata projection
/// so the listing can show date / from / subject.
async fn enrich_metadata(
    client: &Client,
    stubs: Vec<google_gmail::MessageStub>,
) -> anyhow::Result<Vec<Message>> {
    let mut out = Vec::with_capacity(stubs.len());
    for stub in stubs {
        let message = client
            .get_message(&stub.id, MessageFormat::Metadata)
            .await?;
        out.push(message);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::guess_content_type;

    #[test]
    fn unknown_extensions_fall_back_to_octet_stream() {
        assert_eq!(guess_content_type("blob"), "application/octet-stream");
        assert_eq!(guess_content_type("note.xyz"), "application/octet-stream");
    }

    #[test]
    fn common_extensions_map_to_well_known_types() {
        assert_eq!(guess_content_type("report.pdf"), "application/pdf");
        assert_eq!(guess_content_type("photo.JPG"), "image/jpeg");
        assert_eq!(guess_content_type("data.csv"), "text/csv");
    }
}
