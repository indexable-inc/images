# Body of the `say-detached` bash app (see users/andrewgazelka/home.nix).
# No shebang / `set` line: the writeBashApplication wrapper supplies bash + `set -euo
# pipefail` and bakes minecraft-sound + the speaker onto PATH via runtimeInputs.
#
# Play an optional Minecraft sound, then speak text aloud, in a NEW session
# (POSIX setsid via perl, always at /usr/bin/perl on macOS; perl ships on Linux
# too). `home-manager switch` reloads launchd agents / systemd user units, which
# SIGTERM/KILLs the calling agent's entire process group; running the
# announcement in its own session keeps an in-flight speech from being clipped
# mid-sentence. Invoke this in the FOREGROUND: the caller blocks until speech
# ends (so back-to-back announcements stay ordered, not overlapping), yet this
# process sits in a process group the reload never targets, so it finishes even
# if the caller is torn down. Shared by the ix-downtime watcher.
#
# The speaker command is injected as @SAY_CMD@ at build time: `/usr/bin/say` on
# macOS, a configurable command (default `spd-say`) on Linux. It receives the
# text as a single argument.
#
# Usage: say-detached <sound-id|""> <text...>   (sound-id e.g. note/pling)

sound="$1"
shift

exec /usr/bin/perl -e 'use POSIX qw(setsid); setsid() or exit 1; exec @ARGV or exit 1' \
  -- /bin/sh -c '
    [ -n "$1" ] && { minecraft-sound play "$1" >/dev/null 2>&1 || true; }
    shift
    @SAY_CMD@ "$*" || true
  ' say-detached "$sound" "$@"
