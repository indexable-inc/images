# Guard for cargoUnit's `dylib` crate-type support (ENG-12078).
#
# A `dylib` unit exists for one reason: every consumer in the process shares one
# copy of the crate, and therefore one copy of whatever process-global state it
# holds. hyperion builds `flecs_ecs` this way because two copies of flecs's
# component-index pool index one world two different ways, and the symptom is a
# hot reload that silently corrupts rather than a build that fails.
#
# So the assertion is the runtime one. The fixture's engine holds a counter; the
# host bumps it, then `dlopen`s a module that bumps it, then reads it. One
# shared engine gives 0, 1, 2. Two static copies give 0, 0, 1 -- and every other
# thing about that build looks correct, which is why this is a check and not a
# code comment.
#
# What it caught before the support existed: cargoUnit published one
# `nix-support/extern-path` per unit, preferring the rlib, so every consumer of
# a `["rlib", "dylib"]` crate linked it statically. `cargo build -v` over the
# same crate passes rustc TWO `--extern` flags for the one crate name, the rlib
# and the dylib, and lets rustc pick per consumer. Reproduced by hand before the
# fix: `the module saw its own engine copy: 0 instead of 1`.
{
  lib,
  pkgs,
  ix,
}: let
  # An explicit file set rather than the bare directory: a developer who runs
  # cargo inside the fixture leaves a `target/` behind, and taking the directory
  # would fold it into the source hash.
  fixture = lib.fileset.toSource {
    root = ./fixtures/cargo-unit-dylib;
    fileset = lib.fileset.unions [
      ./fixtures/cargo-unit-dylib/Cargo.lock
      ./fixtures/cargo-unit-dylib/Cargo.toml
      ./fixtures/cargo-unit-dylib/engine
      ./fixtures/cargo-unit-dylib/host
      ./fixtures/cargo-unit-dylib/module
    ];
  };

  # The default cargoUnit toolchain, named explicitly because the sysroot rpath
  # below has to point at this exact one.
  rustToolchain = ix.repoRustToolchainFor pkgs {};
  sysrootLib = "${rustToolchain}/lib/rustlib/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/lib";

  workspace = ix.cargoUnit.buildWorkspace {
    inherit rustToolchain;
    pname = "cargo-unit-dylib";
    src = fixture;
    workspaceRoot = ./fixtures/cargo-unit-dylib;
    cargoArgs = ["--workspace"];

    # `-C prefer-dynamic` is the caller's half of dylib support and cannot be
    # inferred: generating a dylib without it makes rustc statically absorb
    # every dependency into the dylib, and linking an executable without it
    # makes rustc prefer the rlib of a crate that offers both. It is a
    # workspace-wide flag here for the same reason hyperion sets it
    # workspace-wide: the crates on both sides of the `dlopen` boundary have to
    # agree about which image owns the engine.
    extraRustcArgs = ["-Cprefer-dynamic"];

    # The other half the caller owns. `-C prefer-dynamic` also makes libstd
    # dynamic, so a linked artifact asks the loader for
    # `libstd-<hash>.so`/`@rpath/libstd-<hash>.dylib` and finds nothing without
    # this. cargoUnit adds the rpaths for dylibs inside the graph, whose store
    # paths only it knows; the toolchain's own lib dir is the caller's, so that
    # a workspace that never asked for prefer-dynamic does not drag the Rust
    # toolchain into its runtime closure.
    extraLinkRustcArgsForPlatform = _platform: ["-Clink-arg=-Wl,-rpath,${sysrootLib}"];

    policy = {
      clippy.enable = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      denyUnusedCrateDependencies = false;
    };
  };

  libraryOrThrow = name:
    workspace.libraries.${
      name
    }
    or (throw ''
      the cargo-unit-dylib fixture has no library target `${name}`.
      Got: ${lib.concatStringsSep ", " (lib.attrNames workspace.libraries)}
    '');

  # Underscored: `libraries` is keyed by the Cargo *target* name, and a default
  # lib target's name is the package name with dashes folded to underscores.
  engine = libraryOrThrow "cargo_unit_dylib_engine";
  module = libraryOrThrow "cargo_unit_dylib_module";
  host = workspace.binaries.cargo-unit-dylib-host;

  # Mach-O keeps its dynamic symbols in a different table than ELF, so the two
  # platforms need different readers for the same question.
  exportedSymbols =
    if pkgs.stdenv.hostPlatform.isElf
    then "nm --dynamic --defined-only"
    else "nm -gU";
in
  pkgs.runCommand "cargo-unit-dylib" {
    nativeBuildInputs = [pkgs.binutils];
  } ''
    # Explicit rather than inherited: the host's own exit status is the primary
    # assertion and it is piped through `tee`, so without `pipefail` a panicking
    # host would leave the pipeline reporting tee's success.
    set -euo pipefail

    engine_dylib=$(echo ${engine}/lib/lib*engine*${pkgs.stdenv.hostPlatform.extensions.sharedLibrary})
    module_dylib=$(echo ${module}/lib/lib*module*${pkgs.stdenv.hostPlatform.extensions.sharedLibrary})
    for artifact in "$engine_dylib" "$module_dylib"; do
      if [ ! -f "$artifact" ]; then
        echo "a dylib unit produced no shared library: $artifact" >&2
        ls -la ${engine}/lib ${module}/lib >&2
        exit 1
      fi
    done

    # The property. Anything other than 0/1/2 means the module is bumping its
    # own copy of the engine's counter.
    ${host}/bin/cargo-unit-dylib-host "$module_dylib" | tee result.txt
    grep -Fxq 'one engine: host=0 module=1 total=2' result.txt

    # One engine image, named by both sides. The rpath cargoUnit emits is what
    # makes the loader agree with the linker; without it the host resolves the
    # engine through whatever else is on the search path, or not at all.
    for artifact in ${host}/bin/cargo-unit-dylib-host "$module_dylib"; do
      if ! grep -Fq ${engine} "$artifact"; then
        echo "$artifact carries no rpath to the engine unit ${engine}" >&2
        exit 1
      fi
    done

    # A symbol absorbed from a native static archive, promoted back out of the
    # dylib by a `cargo::rustc-link-arg` version script from the engine's
    # build.rs. rustc's own anonymous script ends `local: *` and would otherwise
    # demote it; hyperion re-exports LMDB's `mdb_*` through exactly this path,
    # and the failure mode is an undefined symbol at a consumer's final link.
    ${exportedSymbols} "$engine_dylib" > engine-dynsyms.txt
    if ! grep -q cargo_unit_probe engine-dynsyms.txt; then
      echo "the engine dylib does not export cargo_unit_probe." >&2
      echo "A build script's rustc-link-arg is not reaching the dylib unit's link." >&2
      exit 1
    fi

    mkdir -p "$out"
    cp result.txt engine-dynsyms.txt "$out/"
  ''
