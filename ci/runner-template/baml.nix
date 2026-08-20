# Toolchain-baked runner policy for baml-class pools, layered ON
# platform.nix (the flake's `baml` attr imports module.nix + platform.nix +
# this file; never this file alone). Ported from
# github.com/indexable-inc/ix-runners branch pool-mode-v2,
# pools/baml/ci-runner.nix, reduced to the delta over platform.nix: the
# substrate facts that file restated (cache substitution + nix.settings GC
# headroom, gai.conf v4 preference, build-essential parity packages,
# CARGO/NEXTEST/RUST_TEST/VITEST parallelism pins, MISE_*_COMPILE,
# stateVersion) already live in platform.nix with the same values, so they
# are deliberately not repeated here - platform.nix's jobEnvironment values
# are mkDefault, so this layer could still override them in nix if a pool
# ever needs to.
#
# Toolchain provisioning stays rustup + mise (per baml's repo-pinned
# rust-toolchain.toml and mise.toml), not nix: what is listed here is
# Ubuntu-image parity - things GitHub's hosted images preinstall that
# neither rustup nor mise provides on a NixOS guest - plus the env those
# FHS-flavored tools need to find nix's split-output openssl.
{
  lib,
  pkgs,
  ...
}: {
  services.ix-runner = {
    # Delta over platform.nix's build-essential set (gcc, gnumake, cmake,
    # pkg-config, python3, perl, openssl, git-lfs, glibc.bin are already
    # there; extraPackages is a list option that concatenates across
    # modules, so restating them would put duplicate store paths on the
    # job PATH).
    extraPackages = with pkgs; [
      rustup # jobs run `rustup show` to pull the repo-pinned toolchain
      # DEVIATION from the pool-mode-v2 source, which did not bake sccache
      # (mise.toml pinned it, so it arrived with the seed snapshot's HOME):
      # baked here because the first live baml lanes on the platform
      # template died "sccache: command not found" before any
      # mise-installed copy existed. A HOME-installed mise shim still wins
      # on PATH order when present.
      sccache
      ninja # cmake generator some engine builds select
      ruby # release-metadata packaging tests
      go # sdkgen_go's build script shells out to gofmt
      nodejs_22 # pyright runs on the PATH node
      # musl leg: the full cross gcc, under the name setup-musl-cross
      # probes for (the thin musl libc wrapper links broken static-PIE
      # binaries).
      (writeShellScriptBin "musl-gcc" ''
        exec ${pkgsCross.musl64.stdenv.cc}/bin/x86_64-unknown-linux-musl-gcc "$@"
      '')
    ];

    # Delta over platform.nix's pins: only the baml-specific env. attrsOf
    # merge keeps platform.nix's parallelism/mise keys alongside these.
    jobEnvironment = {
      # Playwright's host check refuses NixOS; the browsers run fine via
      # nix-ld's library set.
      PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS = "true";
      # pyright must use the PATH node above, not download its own.
      PYRIGHT_PYTHON_GLOBAL_NODE = "1";
      # openssl-sys (baml_language workspace) probes these; nix splits the
      # outputs Ubuntu ships together.
      OPENSSL_LIB_DIR = "${lib.getLib pkgs.openssl}/lib";
      OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
      PKG_CONFIG_PATH = lib.makeSearchPath "lib/pkgconfig" [pkgs.openssl.dev];
      # The link line above resolves at runtime too: test binaries link
      # libssl dynamically against the nix openssl, and without this the
      # loader answers "libssl.so.3: cannot open shared object file" (hit
      # live when the binaries first ran outside a nix shell).
      LD_LIBRARY_PATH = "${lib.getLib pkgs.openssl}/lib";
      # setup-dotnet defaults to /usr/share/dotnet, read-only here. The
      # module's one runner user homes at /home/runner, and HOME is what
      # the seed snapshot carries - so the runtime installed by a green run
      # is already in place on every later fork of the lineage.
      DOTNET_INSTALL_DIR = "/home/runner/.dotnet";
    };
  };
}
