//! Per-node deployment flow: build the closure locally, copy it, then
//! activate the way `darwin-rebuild switch` does: set the system profile,
//! run legacy `activate-user` if the generation still ships a live one, then
//! run `activate` as root.

use anyhow::{Context, Result, bail};

use crate::exec;
use crate::node::{NodeSpec, Target};
use crate::plan::{self, Installable, Invocation, RunAs};
use crate::report::NodeReport;

const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system";

/// Deploy one node, folding any failure into its report so parallel nodes
/// never mask each other.
pub fn node(flake: &str, spec: &NodeSpec, dry_run: bool) -> NodeReport {
    let mut report = NodeReport::new(spec);
    match flow(flake, spec, dry_run, &mut report) {
        Ok(()) => report.ok = true,
        Err(error) => {
            eprintln!("[{}] FAILED: {error:#}", spec.name);
            report.error = Some(format!("{error:#}"));
        }
    }
    report
}

fn flow(flake: &str, spec: &NodeSpec, dry_run: bool, report: &mut NodeReport) -> Result<()> {
    let name = &spec.name;
    let target = &spec.target;

    let installable = Installable::darwin_system(flake, name);
    let rendered = installable.render()?;
    eprintln!("[{name}] building {rendered}");
    let built = exec::succeed(&plan::build(&installable)?)
        .with_context(|| format!("building {rendered}"))?;
    let Some(system) = built.lines().last().map(str::to_owned) else {
        bail!("`nix build` printed no out path for {rendered}");
    };
    report.system = Some(system.clone());

    let previous = current_system(target).context("reading the current system")?;
    let changed = previous.as_deref() != Some(system.as_str());
    report.changed = Some(changed);
    report.previous = previous;

    if dry_run {
        if changed {
            eprintln!(
                "[{name}] would activate {system} (currently {})",
                report.previous.as_deref().unwrap_or("not activated")
            );
        } else {
            eprintln!("[{name}] up to date at {system}");
        }
        return Ok(());
    }

    eprintln!("[{name}] copying closure to {}", target.store_url());
    exec::succeed(&plan::copy(target, &system)?).context("copying the closure")?;

    eprintln!("[{name}] setting the system profile");
    exec::succeed(&plan::remote(
        target,
        RunAs::Root,
        &["nix-env", "--profile", SYSTEM_PROFILE, "--set", &system],
    )?)
    .context("setting the system profile")?;

    if legacy_activate_user(target, &system)? {
        eprintln!("[{name}] running activate-user");
        exec::succeed(&plan::remote(
            target,
            RunAs::SshUser,
            &[&format!("{system}/activate-user")],
        )?)
        .context("running activate-user")?;
    }

    eprintln!("[{name}] activating");
    exec::succeed(&plan::remote(
        target,
        RunAs::Root,
        &[&format!("{system}/activate")],
    )?)
    .context("activating")?;

    eprintln!("[{name}] activated {system}");
    Ok(())
}

/// The store path `/run/current-system` points at, or `None` on a host that
/// has never activated a generation.
fn current_system(target: &Target) -> Result<Option<String>> {
    let invocation = plan::remote(
        target,
        RunAs::SshUser,
        &["readlink", "/run/current-system"],
    )?;
    let completed = exec::run(&invocation)?;
    match completed.code {
        0 => Ok(Some(completed.stdout.trim().to_owned())),
        1 => Ok(None),
        code => bail!("`{invocation}` exited {code}: {}", completed.stderr.trim()),
    }
}

/// Whether the copied generation still ships a live legacy `activate-user`.
/// Modern nix-darwin keeps a stub marked `# nix-darwin: deprecated`, which
/// `darwin-rebuild` skips, and so do we.
fn legacy_activate_user(target: &Target, system: &str) -> Result<bool> {
    let activate_user = format!("{system}/activate-user");
    if !probe(&plan::remote(target, RunAs::SshUser, &["test", "-x", &activate_user])?)? {
        return Ok(false);
    }
    let deprecated = probe(&plan::remote(
        target,
        RunAs::SshUser,
        &["grep", "-q", "^# nix-darwin: deprecated$", &activate_user],
    )?)?;
    Ok(!deprecated)
}

/// Run a yes/no remote check: exit 0 is yes, exit 1 is no, anything else
/// (including ssh's own 255) is a hard error.
fn probe(invocation: &Invocation) -> Result<bool> {
    let completed = exec::run(invocation)?;
    match completed.code {
        0 => Ok(true),
        1 => Ok(false),
        code => bail!("`{invocation}` exited {code}: {}", completed.stderr.trim()),
    }
}
