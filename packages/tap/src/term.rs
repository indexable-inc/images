//! Controlling-terminal helpers for the attach client: window size and raw mode.
//!
//! Raw mode is wrapped in [`RawGuard`], which restores the original terminal
//! settings on drop, so every exit path (detach, session end, error, panic
//! unwind) leaves the user's shell usable. The original tap restored only on the
//! happy path; an RAII guard removes that whole class of "terminal left in raw
//! mode" bugs.

use std::os::fd::BorrowedFd;

use nix::sys::termios::{SetArg, Termios};

const STDIN_FD: std::os::fd::RawFd = nix::libc::STDIN_FILENO;

/// Current controlling-terminal size as `(rows, cols)`.
///
/// Falls back to 80×24 when stdin is not a terminal or the ioctl fails, so a
/// session started without a tty still has a sane geometry.
#[must_use]
pub fn current_winsize() -> (u16, u16) {
    // SAFETY: `ws` is fully written by a successful TIOCGWINSZ; on failure we
    // discard it and use the fallback.
    let mut ws: nix::libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { nix::libc::ioctl(STDIN_FD, nix::libc::TIOCGWINSZ, &raw mut ws) };
    if ret != 0 || ws.ws_row == 0 || ws.ws_col == 0 {
        return (24, 80);
    }
    (ws.ws_row, ws.ws_col)
}

/// Raw-mode guard for stdin. Restores the original termios on drop.
pub struct RawGuard {
    original: Termios,
}

impl RawGuard {
    /// Put stdin into raw mode, returning a guard that restores it on drop.
    ///
    /// Returns `None` when stdin is not a terminal (for example under a test
    /// harness or a pipe), in which case there is nothing to restore.
    #[must_use]
    pub fn enter() -> Option<Self> {
        let fd = unsafe { BorrowedFd::borrow_raw(STDIN_FD) };
        let original = nix::sys::termios::tcgetattr(fd).ok()?;
        let mut raw = original.clone();
        nix::sys::termios::cfmakeraw(&mut raw);
        nix::sys::termios::tcsetattr(fd, SetArg::TCSANOW, &raw).ok()?;
        Some(Self { original })
    }

    /// Temporarily restore cooked mode (used while a child editor owns the tty).
    pub fn suspend(&self) {
        let fd = unsafe { BorrowedFd::borrow_raw(STDIN_FD) };
        let _ = nix::sys::termios::tcsetattr(fd, SetArg::TCSANOW, &self.original);
    }

    /// Re-enter raw mode after [`suspend`](Self::suspend).
    pub fn resume(&self) {
        let fd = unsafe { BorrowedFd::borrow_raw(STDIN_FD) };
        let mut raw = self.original.clone();
        nix::sys::termios::cfmakeraw(&mut raw);
        let _ = nix::sys::termios::tcsetattr(fd, SetArg::TCSANOW, &raw);
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        self.suspend();
    }
}
