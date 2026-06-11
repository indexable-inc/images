//! Instagram-style "stories" for Claude Code.
//!
//! - `publish` derives your current story from the local repo and writes it to a
//!   state file (wire it to a `SessionStart` hook).
//! - `serve` exposes that file over HTTP so tailnet peers can read it.
//! - `render` is the status-line command: it discovers peers, fetches their
//!   stories concurrently, and prints the avatar row.

mod avatar;
mod discovery;
mod story;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use color_eyre::Result;
use color_eyre::eyre::Context;

use crate::discovery::Discovery;
use crate::story::Story;

/// Default port for the per-host story server.
const DEFAULT_PORT: u16 = 4810;

#[derive(Parser)]
#[command(name = "claude-stories", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Derive the current repo's story and write it to the state file.
    Publish {
        /// Repository to read; defaults to the current directory.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Serve the published story over HTTP at `/story`.
    Serve {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Address to bind. Defaults to all interfaces so tailnet peers reach it.
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
    },
    /// Render the status-line row of peers' stories.
    Render {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
    },
    /// Print the current repo's story as JSON (for debugging).
    Show {
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    match cli.command {
        Command::Publish { path } => publish(path),
        Command::Serve { port, bind } => serve(&bind, port).await,
        Command::Render { port } => render(port).await,
        Command::Show { path } => show(path),
    }
}

fn cwd_or(path: Option<PathBuf>) -> Result<PathBuf> {
    path.map_or_else(
        || std::env::current_dir().wrap_err("reading current directory"),
        Ok,
    )
}

fn publish(path: Option<PathBuf>) -> Result<()> {
    let story = story::derive(&cwd_or(path)?)?;
    story::write_state(&story)?;
    println!(
        "published: {} in {}@{}",
        story.name, story.repo, story.branch
    );
    Ok(())
}

fn show(path: Option<PathBuf>) -> Result<()> {
    let story = story::derive(&cwd_or(path)?)?;
    println!("{}", serde_json::to_string_pretty(&story)?);
    Ok(())
}

async fn serve(bind: &str, port: u16) -> Result<()> {
    use axum::Router;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::get;

    async fn story_handler() -> impl IntoResponse {
        match story::read_state() {
            Ok(Some(s)) => axum::Json(s).into_response(),
            Ok(None) => (StatusCode::NOT_FOUND, "no story published").into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    let app = Router::new().route("/story", get(story_handler));
    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .wrap_err_with(|| format!("binding {addr}"))?;
    println!("claude-stories serving on http://{addr}/story");
    axum::serve(listener, app).await.wrap_err("serving")?;
    Ok(())
}

async fn render(port: u16) -> Result<()> {
    let endpoints = Discovery::from_env().endpoints(port)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .wrap_err("building HTTP client")?;

    let mut set = tokio::task::JoinSet::new();
    for url in endpoints {
        let client = client.clone();
        set.spawn(async move { fetch_story(&client, &url).await });
    }

    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let mut stories: Vec<Story> = Vec::new();
    while let Some(joined) = set.join_next().await {
        // A peer that is offline, slow, or has no story is simply absent from
        // the row; that is the expected steady state, not an error.
        if let Ok(Some(s)) = joined
            && s.is_fresh(now)
        {
            stories.push(s);
        }
    }

    println!("{}", avatar::row(stories));
    Ok(())
}

async fn fetch_story(client: &reqwest::Client, url: &str) -> Option<Story> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Story>().await.ok()
}
