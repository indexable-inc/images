//! Typed command plans. Every fact (flake ref, attr name, ssh destination,
//! store path) stays a typed binding until one renderer serializes it at the
//! boundary its format owns: nix installables, `ssh://` store URLs, and the
//! shell-quoted remote command line ssh delivers.

use std::fmt;

use anyhow::{Context, Result, bail};

use crate::node::Target;

/// Options every ssh connection gets, both our own `ssh` argv and (via
/// `NIX_SSHOPTS`) the ssh processes `nix copy` spawns: fail loudly instead of
/// hanging on an interactive password prompt.
const SSH_OPTIONS: [&str; 2] = ["-o", "BatchMode=yes"];

/// A fully rendered local process invocation: program plus argv, never a
/// shell string.
pub struct Invocation {
    pub program: &'static str,
    pub args: Vec<String>,
    pub env: Vec<EnvVar>,
}

pub struct EnvVar {
    pub name: &'static str,
    pub value: String,
}

impl fmt::Display for Invocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.program)?;
        for arg in &self.args {
            match shlex::try_quote(arg) {
                Ok(quoted) => write!(formatter, " {quoted}")?,
                // A nul byte cannot be shell-quoted; this rendering is
                // diagnostics-only (never executed), so show the arg debug-quoted.
                Err(_) => write!(formatter, " {arg:?}")?,
            }
        }
        Ok(())
    }
}

/// A flake output attribute: flake ref plus attr path, rendered as one
/// `<flake>#<attr.path>` installable.
pub struct Installable {
    flake_ref: String,
    attr_path: Vec<String>,
}

impl Installable {
    /// The system closure of `darwinConfigurations.<name>` (what
    /// `darwin-rebuild build --flake` builds).
    pub fn darwin_system(flake_ref: &str, name: &str) -> Self {
        Self {
            flake_ref: flake_ref.to_owned(),
            attr_path: vec![
                "darwinConfigurations".to_owned(),
                name.to_owned(),
                "system".to_owned(),
            ],
        }
    }

    pub fn render(&self) -> Result<String> {
        let elements: Vec<String> = self
            .attr_path
            .iter()
            .map(|element| attr_element(element))
            .collect::<Result<_>>()?;
        Ok(format!("{}#{}", self.flake_ref, elements.join(".")))
    }
}

/// Render one attr-path element the way nix's installable parser reads it:
/// bare identifiers pass through, anything else is double-quoted. The parser
/// has no escape sequences inside quotes, so a name containing `"` cannot be
/// expressed at all.
fn attr_element(name: &str) -> Result<String> {
    let bare = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '\''));
    if bare {
        return Ok(name.to_owned());
    }
    if name.contains('"') {
        bail!("attribute name `{name}` contains `\"`, which nix attr paths cannot quote");
    }
    Ok(format!("\"{name}\""))
}

/// Build the system closure locally and print its out path.
pub fn build(installable: &Installable) -> Result<Invocation> {
    Ok(Invocation {
        program: "nix",
        args: vec![
            "build".to_owned(),
            "--no-link".to_owned(),
            "--print-out-paths".to_owned(),
            installable.render()?,
        ],
        env: Vec::new(),
    })
}

/// Render `NIX_SSHOPTS`: nix tokenizes it on plain whitespace (no shell
/// unquoting), so the only representable options are whitespace-free tokens
/// joined by spaces; anything else must fail here, not silently misparse.
fn nix_ssh_opts() -> Result<String> {
    for option in SSH_OPTIONS {
        if option.chars().any(char::is_whitespace) {
            bail!("ssh option `{option}` contains whitespace, which NIX_SSHOPTS cannot carry");
        }
    }
    Ok(SSH_OPTIONS.join(" "))
}

/// Copy the closure to the target's store over ssh.
pub fn copy(target: &Target, store_path: &str) -> Result<Invocation> {
    let ssh_options = nix_ssh_opts()?;
    Ok(Invocation {
        program: "nix",
        args: vec![
            "copy".to_owned(),
            "--to".to_owned(),
            target.store_url(),
            store_path.to_owned(),
        ],
        env: vec![EnvVar {
            name: "NIX_SSHOPTS",
            value: ssh_options,
        }],
    })
}

/// Who a remote command runs as.
#[derive(Clone, Copy)]
pub enum RunAs {
    /// Prefixed with `sudo --set-home --` unless the ssh user is already root.
    Root,
    SshUser,
}

/// Render `argv` into one ssh invocation against `target`. This is the single
/// place a remote command becomes a shell string.
pub fn remote(target: &Target, run_as: RunAs, argv: &[&str]) -> Result<Invocation> {
    let mut words: Vec<&str> = Vec::new();
    if matches!(run_as, RunAs::Root) && !target.is_root() {
        words.extend(["sudo", "--set-home", "--"]);
    }
    words.extend(argv);
    let command = shlex::try_join(words).context("rendering remote command")?;

    let mut ssh_args: Vec<String> =
        SSH_OPTIONS.iter().map(|option| (*option).to_owned()).collect();
    ssh_args.push(target.ssh_destination());
    ssh_args.push(command);
    Ok(Invocation {
        program: "ssh",
        args: ssh_args,
        env: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(destination: &str) -> Target {
        destination.parse().expect("valid destination")
    }

    #[test]
    fn renders_plain_installable() {
        let installable = Installable::darwin_system(".", "mac1");
        assert_eq!(
            installable.render().expect("renders"),
            ".#darwinConfigurations.mac1.system"
        );
    }

    #[test]
    fn quotes_non_identifier_attr_names() {
        let installable = Installable::darwin_system("github:owner/repo", "ci runner.2");
        assert_eq!(
            installable.render().expect("renders"),
            "github:owner/repo#darwinConfigurations.\"ci runner.2\".system"
        );
    }

    #[test]
    fn rejects_unquotable_attr_names() {
        let installable = Installable::darwin_system(".", "bad\"name");
        assert!(installable.render().is_err());
    }

    #[test]
    fn builds_the_system_closure() {
        let invocation =
            build(&Installable::darwin_system(".", "mac1")).expect("renders");
        assert_eq!(invocation.program, "nix");
        assert_eq!(
            invocation.args,
            [
                "build",
                "--no-link",
                "--print-out-paths",
                ".#darwinConfigurations.mac1.system"
            ]
        );
    }

    #[test]
    fn copies_through_a_batch_mode_ssh_store() {
        let invocation =
            copy(&target("admin@mac1.local"), "/nix/store/abc-system").expect("renders");
        assert_eq!(invocation.program, "nix");
        assert_eq!(
            invocation.args,
            ["copy", "--to", "ssh://admin@mac1.local", "/nix/store/abc-system"]
        );
        assert_eq!(invocation.env.len(), 1);
        assert_eq!(invocation.env[0].name, "NIX_SSHOPTS");
        assert_eq!(invocation.env[0].value, "-o BatchMode=yes");
    }

    #[test]
    fn remote_root_commands_gain_sudo() {
        let invocation = remote(
            &target("admin@mac1.local"),
            RunAs::Root,
            &["nix-env", "--profile", "/nix/var/nix/profiles/system", "--set", "/nix/store/abc"],
        )
        .expect("renders");
        assert_eq!(invocation.program, "ssh");
        assert_eq!(
            invocation.args,
            [
                "-o",
                "BatchMode=yes",
                "admin@mac1.local",
                "sudo --set-home -- nix-env --profile /nix/var/nix/profiles/system --set /nix/store/abc"
            ]
        );
    }

    #[test]
    fn remote_root_commands_skip_sudo_for_root() {
        let invocation =
            remote(&target("root@mac1.local"), RunAs::Root, &["/nix/store/abc/activate"])
                .expect("renders");
        assert_eq!(
            invocation.args,
            ["-o", "BatchMode=yes", "root@mac1.local", "/nix/store/abc/activate"]
        );
    }

    #[test]
    fn remote_commands_shell_quote_arguments() {
        let invocation = remote(
            &target("mac1.local"),
            RunAs::SshUser,
            &["grep", "-q", "^# nix-darwin: deprecated$", "/nix/store/abc/activate-user"],
        )
        .expect("renders");
        assert_eq!(
            invocation.args,
            [
                "-o",
                "BatchMode=yes",
                "mac1.local",
                "grep -q '^# nix-darwin: deprecated$' /nix/store/abc/activate-user"
            ]
        );
    }

    #[test]
    fn displays_a_quoted_command_line() {
        let invocation = remote(&target("mac1.local"), RunAs::SshUser, &["readlink", "/run/current-system"])
            .expect("renders");
        assert_eq!(
            invocation.to_string(),
            "ssh -o 'BatchMode=yes' mac1.local 'readlink /run/current-system'"
        );
    }
}
