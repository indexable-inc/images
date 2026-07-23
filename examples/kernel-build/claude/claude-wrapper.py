"""Root's entry point to the sandboxed agent, installed on PATH as `claude`.

Drops from the operator's root shell into the unprivileged `claude` user via
systemd-run (a fresh cgroup-scoped pty session in the caller's working
directory) and execs the real Claude Code with its network world rewired:
ANTHROPIC_BASE_URL points at the loopback key-injecting proxy and the API
key env var is a decoy. NoNewPrivileges pins the uid for every descendant,
which is exactly the identity the claude-egress nftables policy keys on.

argv prefix (wired by claude.nix): SYSTEMD_RUN CLAUDE_BIN BASE_URL PATH
CPATH LIBRARY_PATH PKG_CONFIG_PATH, then the operator's own claude args.
"""

import os
import sys


def main() -> None:
    (
        _self,
        systemd_run,
        claude_bin,
        base_url,
        path,
        cpath,
        library_path,
        pkg_config_path,
        *claude_args,
    ) = sys.argv

    if os.getuid() != 0:
        sys.stderr.write(
            "claude: run this from the VM's root shell (ix shell); "
            "it drops to the sandboxed 'claude' user itself\n"
        )
        raise SystemExit(1)

    env = {
        "HOME": "/home/claude",
        # Transient units start from systemd's compiled-in default PATH,
        # which points nowhere on NixOS.
        "PATH": path,
        "TERM": os.environ.get("TERM", "xterm-256color"),
        # The kbuild host-tool search paths from toolchain.nix, so the
        # agent's build commands see the same environment a human shell does.
        "CPATH": cpath,
        "LIBRARY_PATH": library_path,
        "PKG_CONFIG_PATH": pkg_config_path,
        "ANTHROPIC_BASE_URL": base_url,
        # Never the real key; the proxy strips this and injects its own.
        "ANTHROPIC_API_KEY": "dummy-key-the-proxy-injects-the-real-one",
        # Skip telemetry/update probes that would only burn timeouts against
        # the egress policy.
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    }

    argv = [
        systemd_run,
        "--quiet",
        "--collect",
        "--wait",
        "--pty",
        "--same-dir",
        "--uid=claude",
        "--gid=claude",
        "--property=NoNewPrivileges=yes",
        *[f"--setenv={name}={value}" for name, value in env.items()],
        claude_bin,
        *claude_args,
    ]
    # Replacing this process (no shell involved) is the point: the wrapper
    # vanishes and systemd-run owns the pty. Hence the S606 exemption.
    os.execv(argv[0], argv)  # noqa: S606


if __name__ == "__main__":
    main()
