"""Regenerate the committed `.ix` converter, lib/ix2nix.wasm.

`lib.importIxWasm` loads that committed file with zero mid-eval store
realization, and the `ix2nix-wasm-fresh` check byte-compares it against the
built `.#ix2nix-wasm`, so a converter source change (or any edit under
packages/ix2nix, whose whole directory is the ix2nix unit source and leaks
into two embedded panic-location paths) lands together with a rerun of this
script. The artifact lives under lib/, outside its own build's inputs, so
one rerun always converges.

x86_64-linux on purpose: the artifact is not bit-identical across build
hosts (the toolchain store path feeds `-C metadata`), so the committed bytes
are pinned to the system the CI freshness gate builds; see
packages/ix2nix/wasm/default.nix. A darwin host needs a linux remote builder
or a warm cache for this build. `nix` comes from the ambient PATH so the
client matches the host daemon.
"""

import shutil
import subprocess
from pathlib import Path


def run(argv: list[str], cwd: Path | None = None) -> str:
    result = subprocess.run(argv, check=True, capture_output=True, text=True, cwd=cwd)
    return result.stdout.strip()


def main() -> None:
    root = Path(run(["git", "rev-parse", "--show-toplevel"]))
    out = run(
        [
            "nix",
            "build",
            ".#packages.x86_64-linux.ix2nix-wasm",
            "--no-link",
            "--print-out-paths",
            "--option",
            "extra-experimental-features",
            "ca-derivations",
        ],
        cwd=root,
    )
    target = root / "lib" / "ix2nix.wasm"
    shutil.copyfile(Path(out) / "lib" / "ix2nix.wasm", target)
    target.chmod(0o644)
    print(f"regenerated {target}; review and commit it")


if __name__ == "__main__":
    main()
