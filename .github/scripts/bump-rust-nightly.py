#!/usr/bin/env python3
"""Bump rust-toolchain.toml to the latest Rust nightly, if it is safe to do so.

Run from the repo root. Rewrites the `channel` line in place when a newer
nightly is available AND every component/target this repo's
rust-toolchain.toml requires is present in that nightly's manifest. Exits 0
(no file change) when there's nothing to bump or the newer nightly is
missing something this repo needs -- create-pull-request no-ops on an empty
diff, so a no-op run here just means no PR (or an existing rolling PR is left
untouched).
"""

import re
import sys
import tomllib
import urllib.request

MANIFEST_URL = "https://static.rust-lang.org/dist/channel-rust-nightly.toml"
TOOLCHAIN_PATH = "rust-toolchain.toml"

# rustup's component short names (as written in rust-toolchain.toml) don't
# always match the manifest's package keys: some ship as "-preview" packages
# upstream even though rustup exposes them under the short name.
COMPONENT_PKG_NAME = {
    "rust-src": "rust-src",
    "rust-analyzer": "rust-analyzer-preview",
    "rustc-dev": "rustc-dev",
    "llvm-tools": "llvm-tools-preview",
}

# Host platforms this repo's CI actually builds the toolchain on. A `targets`
# entry in rust-toolchain.toml is a cross-compile target: rustup only needs
# `rust-std` for it, not every component, so it is checked separately below.
HOST_PLATFORMS = ["x86_64-unknown-linux-musl", "aarch64-apple-darwin"]


def main() -> int:
    with open(TOOLCHAIN_PATH, "rb") as f:
        toolchain = tomllib.load(f)["toolchain"]

    current_channel = toolchain["channel"]
    m = re.fullmatch(r"nightly-(\d{4}-\d{2}-\d{2})", current_channel)
    if not m:
        print(f"channel {current_channel!r} is not a dated nightly pin, nothing to do")
        return 0
    current_date = m.group(1)

    components = toolchain.get("components", [])
    targets = toolchain.get("targets", [])

    with urllib.request.urlopen(MANIFEST_URL, timeout=30) as resp:
        manifest = tomllib.loads(resp.read().decode())

    manifest_date = manifest["date"]
    if manifest_date <= current_date:
        print(f"pinned {current_date} is already >= manifest {manifest_date}, nothing to do")
        return 0

    pkgs = manifest["pkg"]

    # Every component must be available for every host platform this repo
    # builds on, or the bump would break that platform's toolchain fetch.
    for component in components:
        pkg_name = COMPONENT_PKG_NAME.get(component, component)
        pkg = pkgs.get(pkg_name)
        if pkg is None:
            print(f"skip: component {component!r} (pkg {pkg_name!r}) not present in manifest {manifest_date}")
            return 0
        pkg_targets = pkg.get("target", {})
        for host in HOST_PLATFORMS:
            info = pkg_targets.get(host) or pkg_targets.get("*")
            if info is None or not info.get("available", False):
                print(f"skip: component {component!r} unavailable for {host} in manifest {manifest_date}")
                return 0

    # A cross-compile target only needs `rust-std` to be available on it.
    rust_std_targets = pkgs["rust-std"]["target"]
    for target in targets:
        info = rust_std_targets.get(target)
        if info is None or not info.get("available", False):
            print(f"skip: target {target!r} has no available rust-std in manifest {manifest_date}")
            return 0

    with open(TOOLCHAIN_PATH, encoding="utf-8") as f:
        content = f.read()
    new_content = content.replace(f'"{current_channel}"', f'"nightly-{manifest_date}"')
    if new_content == content:
        print(f"could not find channel line for {current_channel!r} to rewrite")
        return 1
    with open(TOOLCHAIN_PATH, "w", encoding="utf-8") as f:
        f.write(new_content)

    print(f"bumped nightly {current_date} -> {manifest_date}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
