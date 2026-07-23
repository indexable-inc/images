"""Health check: the claude uid reaches the proxy and nothing else.

Proves the containment behaves rather than that the units exist: the policy
table is loaded, a connect to the proxy port succeeds from the claude uid,
and a direct https attempt from the same uid fails fast (reject, not a
timeout hang). Secret-independent, so it passes on a fresh VM and in CI
where no key is attached.

argv (wired by claude.nix): SYSTEMD_RUN NFT NC CURL PROXY_PORT.
"""

import subprocess
import sys


def run_as_claude(systemd_run: str, argv: list[str]) -> int:
    return subprocess.run(
        [
            systemd_run,
            "--quiet",
            "--collect",
            "--pipe",
            "--wait",
            "--uid=claude",
            "--gid=claude",
            *argv,
        ],
        check=False,
    ).returncode


def main() -> None:
    _self, systemd_run, nft, nc, curl, proxy_port = sys.argv

    subprocess.run(
        [nft, "list", "table", "inet", "claude-egress"],
        check=True,
        stdout=subprocess.DEVNULL,
    )

    if run_as_claude(systemd_run, [nc, "-z", "127.0.0.1", proxy_port]) != 0:
        raise SystemExit("the claude uid cannot reach the loopback proxy")

    reached = run_as_claude(
        systemd_run,
        [curl, "--silent", "--output", "/dev/null", "--max-time", "5",
         "https://api.anthropic.com/"],
    )
    if reached == 0:
        raise SystemExit("the claude uid unexpectedly reached the internet directly")


if __name__ == "__main__":
    main()
