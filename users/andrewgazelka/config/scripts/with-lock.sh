# Body of the `with-lock` writeShellApplication (see home/darwin.nix).
# No shebang / `set` line: writeShellApplication supplies bash + `set -euo pipefail`
# and bakes coreutils onto PATH via runtimeInputs.
#
# Reusable "do not overlap" wrapper for launchd agents (and manual runs). Grabs a
# NON-BLOCKING exclusive lock named by the caller; if the lock is already held
# (a previous run is still going), it exits 0 silently so the scheduled fire is
# skipped instead of overlapping. Otherwise it runs the command and releases the
# lock when it finishes, INCLUDING on crash/kill, because the kernel drops the
# flock the instant the lock-holding process dies.
#
# macOS has no stock `flock(1)` (util-linux is Linux-only), so the lock is taken
# via /usr/bin/perl + flock(2) (perl is always present on macOS). The lock is
# held in the perl PARENT and the child is run with system() (NOT exec): perl
# sets close-on-exec on fds > 2, so exec'ing the child would close the lock fd
# and drop the lock. Holding it in the parent and system()ing the child keeps
# the fd open for the child's whole lifetime; when perl exits, the lock releases.
#
# Usage: with-lock <name> -- <cmd> [args...]
#   e.g. with-lock pr-watch -- /nix/store/.../bin/pr-watch
# Lock dir defaults to ${XDG_CACHE_HOME:-$HOME/.cache}/launchd-locks, overridable
# via $LAUNCHD_LOCK_DIR. Same <name> blocks itself; different names never clash.

name="${1:-}"
[ -n "$name" ] || { echo "with-lock: missing <name>" >&2; exit 2; }
shift

[ "${1:-}" = "--" ] || { echo "with-lock: expected -- before command" >&2; exit 2; }
shift

[ "$#" -gt 0 ] || { echo "with-lock: missing command after --" >&2; exit 2; }

lock_dir="${LAUNCHD_LOCK_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/launchd-locks}"
mkdir -p "$lock_dir"
lockfile="$lock_dir/$name.lock"

exec /usr/bin/perl -MFcntl=:flock -e '
  my $lock = shift;
  open(my $fh, ">", $lock) or exit 1;
  unless (flock($fh, LOCK_EX | LOCK_NB)) { exit 0 }   # held -> skip quietly
  my $rc = system(@ARGV);
  exit($rc == -1 ? 1 : ($rc >> 8));
' "$lockfile" "$@"
