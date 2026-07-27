//! Where the lending socket lives.
//!
//! The path has to be computable by three parties that never talk to each
//! other: the workstation starting the loan, git's helper on the borrowing
//! host, and a long-lived process (the index Elixir kernel) that outlives
//! every ssh session and so cannot inherit a path from one. So it is
//! derived, not negotiated, and `IX_CREDENTIAL_SOCK` overrides it for
//! anyone who needs a second concurrent loan.
//!
//! Per-uid, because the directory on a shared host is world-writable and
//! sticky (`/run/ix-credential`, mode 1777, like `/tmp`): each user binds
//! their own socket, `StreamLocalBindMask 0177` keeps it mode 0600, and the
//! sticky bit stops one user unlinking another's.

use std::path::PathBuf;

/// Overrides the derived path.
pub const SOCKET_ENV: &str = "IX_CREDENTIAL_SOCK";

/// The directory holding per-uid sockets on Linux. Created by the host
/// configuration, because sshd binds a forwarded socket but will not create
/// its parent.
const RUNTIME_DIR: &str = "/run/ix-credential";

/// The socket this process should use.
pub fn path() -> PathBuf {
    if let Some(explicit) = std::env::var_os(SOCKET_ENV) {
        return PathBuf::from(explicit);
    }
    derived(unsafe_uid())
}

/// The derived path for `uid`, with no environment consulted.
fn derived(uid: u32) -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from(RUNTIME_DIR).join(format!("{uid}.sock"))
    } else {
        // macOS has no /run. This is the lending side, a single-user
        // machine, so the per-user temp directory is the right home.
        std::env::temp_dir().join(format!("ix-credential-{uid}.sock"))
    }
}

/// `getuid` never fails and never touches errno, so the FFI call has no
/// failure mode to handle.
fn unsafe_uid() -> u32 {
    // SAFETY: getuid(2) is always successful and takes no arguments.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_derived_path_is_per_uid() {
        assert_ne!(derived(0), derived(1000));
    }

    #[test]
    fn the_derived_path_names_the_uid() {
        let path = derived(1000);
        let name = path.file_name().expect("has a file name").to_string_lossy();
        assert!(name.contains("1000"), "{name}");
        assert!(name.ends_with(".sock"), "{name}");
    }

    #[test]
    fn linux_puts_the_socket_in_the_shared_runtime_dir() {
        if cfg!(target_os = "linux") {
            assert_eq!(derived(0).parent(), Some(std::path::Path::new(RUNTIME_DIR)));
        }
    }
}
