//! Provision a plain base-image VM into a loom control VM. Runs INSIDE the
//! VM, as root: the imperative e2e reference for the declarative
//! `nixosConfigurations.loom` template.
//!
//! Required env:
//!   LOOM_SRC_URL - tarball of index/packages/loom
//!
//! Expects the VM to have been created with the account secrets attached:
//!   --secret-file loom_ix_token=loom_ix_token
//!   --secret-file anthropic_api_key=anthropic_api_key
//!
//! Build for the VM (`nix build`, binary `loom-provision` of this crate,
//! x86_64-linux) and copy it in with `ix push`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::Context as _;

/// The wrapper the children run: key from disk first (the fork wake path),
/// tmpfs as fallback. Generated at runtime inside the VM, where this crate's
/// `loom-claude` binary is not installed.
const CLAUDE_WRAP: &str = r#"#!/bin/sh
key_file=/var/lib/loom/anthropic_api_key
[ -s "$key_file" ] || key_file=/run/secrets/anthropic_api_key
export ANTHROPIC_API_KEY="$(cat "$key_file")"
exec /root/.local/bin/claude "$@"
"#;

/// The human launcher: `loom` inside the VM is a configured iex.
const LOOM_LAUNCH: &str = r#"#!/bin/sh
export PATH=/root/.nix-profile/bin:/root/bin:/usr/bin:/bin
export HOME=/root MIX_ENV=prod
export LOOM_PARENT_VM="${LOOM_PARENT_VM:-loom-ctl}"
export LOOM_IX_BIN=/root/bin/ix
export LOOM_CLAUDE_BIN=/root/bin/claude
export LOOM_PREFLIGHT="test -s /var/lib/loom/anthropic_api_key && test -x /root/bin/claude"
# Same-node hairpin workaround; drop once guests can dial siblings.
export LOOM_IX_PREFIX="${LOOM_IX_PREFIX:---admin}"
export LOOM_RESTORE_ARGS="${LOOM_RESTORE_ARGS:---on hil-compute-2}"
# The fork is the sandbox; in-guest permission prompts protect nothing.
export LOOM_CLAUDE_ARGS="${LOOM_CLAUDE_ARGS:---dangerously-skip-permissions}"
cd /root/loom && exec iex -S mix run --no-deps-check
"#;

fn main() -> ExitCode {
    match run() {
        Ok(()) => {
            println!("loom control VM provisioned");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("loom-provision: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let src_url = loom_launch::required_env("LOOM_SRC_URL")?;
    install_ix_cli()?;
    persist_credentials()?;
    install_claude()?;
    build_loom(&src_url)?;
    write_executable("/root/bin/loom", LOOM_LAUNCH)
}

/// The ix CLI, through the same public installer every customer uses.
fn install_ix_cli() -> anyhow::Result<()> {
    std::fs::create_dir_all("/root/bin").context("mkdir /root/bin")?;
    let mut cmd = sh("curl -fsSL https://ix.dev/install.sh | sh");
    cmd.env("IX_INSTALL_DIR", "/root/.local/bin");
    run_cmd(cmd)?;
    let link = Path::new("/root/bin/ix");
    let _ = std::fs::remove_file(link);
    std::os::unix::fs::symlink("/root/.local/bin/ix", link).context("symlink /root/bin/ix")
}

/// CLI credentials, from the attached secrets. `/run/secrets` is tmpfs and
/// does NOT survive a stop/start of a restored fork (measured live), so
/// everything a fork needs at wake time is persisted to DISK: snapshots
/// capture disk, and disk survives cold boots.
fn persist_credentials() -> anyhow::Result<()> {
    std::fs::create_dir_all("/var/lib/loom").context("mkdir /var/lib/loom")?;
    std::fs::create_dir_all("/root/.config/ix").context("mkdir /root/.config/ix")?;
    let key = std::fs::read("/run/secrets/anthropic_api_key")
        .context("read /run/secrets/anthropic_api_key")?;
    write_secret("/var/lib/loom/anthropic_api_key", &key)?;
    let token = std::fs::read_to_string("/run/secrets/loom_ix_token")
        .context("read /run/secrets/loom_ix_token")?;
    let config = format!(
        "token = \"{}\"\nserver = \"https://api.ix.dev\"\n",
        token.trim_end_matches('\n')
    );
    write_secret("/root/.config/ix/config.toml", config.as_bytes())
}

/// claude through its official installer, plus the on-disk wrapper.
fn install_claude() -> anyhow::Result<()> {
    run_cmd(bash("curl -fsSL https://claude.ai/install.sh | bash"))?;
    write_executable("/root/bin/claude", CLAUDE_WRAP)
}

/// Elixir (cache.ix.dev substitutes it) and loom itself, compiled in place.
fn build_loom(src_url: &str) -> anyhow::Result<()> {
    run_cmd(cmd("nix", &["profile", "install", "nixpkgs#elixir"]))?;
    run_cmd(with_profile_path(cmd("mix", &["local.hex", "--force"])))?;
    let mut fetch = sh("cd /root && rm -rf loom && curl -sf \"$LOOM_SRC_URL\" | tar xz");
    fetch.env("LOOM_SRC_URL", src_url);
    run_cmd(fetch)?;
    let mut compile = with_profile_path(cmd(
        "mix",
        &["compile", "--no-deps-check", "--warnings-as-errors"],
    ));
    compile.current_dir("/root/loom").env("MIX_ENV", "prod");
    run_cmd(compile)
}

fn cmd(program: &str, args: &[&str]) -> Command {
    let mut c = Command::new(program);
    c.args(args);
    c
}

fn sh(script: &str) -> Command {
    let mut c = Command::new("sh");
    c.args(["-c", script]);
    c
}

fn bash(script: &str) -> Command {
    let mut c = Command::new("bash");
    c.args(["-c", script]);
    c
}

/// `nix profile install` lands binaries in the profile, not on PATH.
fn with_profile_path(mut c: Command) -> Command {
    let path = std::env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("/root/.nix-profile/bin:{path}"));
    c
}

fn run_cmd(mut c: Command) -> anyhow::Result<()> {
    let program = c.get_program().to_string_lossy().into_owned();
    let status = c.status().with_context(|| format!("spawn {program}"))?;
    anyhow::ensure!(status.success(), "{program} exited with {status}");
    Ok(())
}

fn write_secret(path: &str, contents: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, contents).with_context(|| format!("write {path}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 600 {path}"))
}

fn write_executable(path: &str, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents).with_context(|| format!("write {path}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("chmod 755 {path}"))
}
