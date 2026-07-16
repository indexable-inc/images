/**
kbuild-unit: per-translation-unit content-addressed Linux kernel builds
(#3411), the kbuild analog of lib/rust/cargo-unit.nix.

Stage 1 (`plan`) runs a full monolithic kbuild under pinned reproducibility
env, then harvests every `.cmd` file plus a snapshot of build-created
non-unit files (kconfig output, generated headers, host tools, linker
scripts) into `plan.json`. Stage 2 renders plan.json to units.nix (IFD),
which builds one CA derivation per unit by replaying its exact saved
command inside a symlink farm of src + snapshot + dep unit outputs.

The plan build's monolithic vmlinux is kept as the equivalence reference:
`vmlinuxEquivalence` cmp's it byte-for-byte against the unit-built vmlinux.

Unit trees currently take the whole kernel source as input, so an edit to
any source rebuilds every unit's tree (CA cutoff still dedups unchanged
outputs); per-unit source scoping is #3412.
*/
{
  lib,
  pkgs,
  nixKbuildUnit,
}: let
  # Toolset the plan kbuild needs; unit replays get the same set so a saved
  # command never resolves a tool differently than the plan build did.
  kbuildInputs = with pkgs; [
    bison
    flex
    bc
    perl
    python3
    openssl
    elfutils
    zlib
    pahole
    kmod
    cpio
  ];

  # Reproducibility pins. #3410 (repro baseline) has no verdict yet, so this
  # uses a fixed epoch timestamp string rather than the empty-string form;
  # reconcile when the baseline lands.
  reproEnv = {
    KBUILD_BUILD_TIMESTAMP = "Thu Jan  1 00:00:00 UTC 1970";
    KBUILD_BUILD_USER = "nixbld";
    KBUILD_BUILD_HOST = "ix";
    KBUILD_BUILD_VERSION = "1";
    # The ld-wrapper otherwise injects `-rpath $out/lib` into shared-object
    # links; the 32-bit vdso is linked as a .so, so the builder's own store
    # path lands inside vmlinux and plan/unit outputs can never match.
    NIX_DONT_SET_RPATH = "1";
  };

  contentAddressing = {
    __contentAddressed = true;
    outputHashAlgo = "sha256";
    outputHashMode = "recursive";
  };

  buildKernel = {
    src,
    configTarget ? "tinyconfig",
    contentAddressed ? true,
  }: let
    # The kernel `src` is a tarball; units and harvest need the unpacked tree
    # as a directory (symlink-farm base and byte-compare reference).
    srcTree = pkgs.srcOnly {
      name = "kbuild-unit-src";
      inherit src;
      # Unpack only; omitting stdenv trips this repo's abort-on-warn.
      stdenv = pkgs.stdenvNoCC;
    };

    plan = pkgs.stdenv.mkDerivation ({
        pname = "kbuild-unit-plan";
        version = configTarget;
        src = srcTree;
        nativeBuildInputs = kbuildInputs ++ [nixKbuildUnit];
        env = reproEnv;
        # Unit replays must see identical bits: no strip / patchelf over the
        # reference vmlinux or the snapshotted host tools.
        dontFixup = true;
        configurePhase = ''
          runHook preConfigure
          # kbuild passes -frandom-seed=<objtree hash> per object; the
          # cc-wrapper appends NIX_CFLAGS_COMPILE after user argv, so the last
          # seed wins. Pin the appended seed to a constant here and in every
          # unit replay (templates/units.nix.in), making the saved command's
          # own seed irrelevant on both sides.
          export NIX_CFLAGS_COMPILE="''${NIX_CFLAGS_COMPILE:-} -frandom-seed=kbuild-unit"
          # Capture the env kbuild exports to scripts/link-vmlinux.sh so the
          # rendered link unit can replay it (CC/LD/LINUXINCLUDE/KBUILD_* for
          # the in-script init/version-timestamp.o compile and postlink make).
          # The dump is build-created and not unit-owned, so harvest sweeps it
          # into the generated snapshot on its own. `export -p` because make
          # runs the script under $(CONFIG_SHELL).
          # `|| :` and the muted stderr keep unit replays quiet: there the
          # dump target is a store symlink from the generated snapshot, the
          # rewrite fails by design, and the sourced snapshot env is already
          # identical to what the dump would produce.
          sed -i '1a { export -p | grep -E "^(declare -x |export )(KBUILD_[A-Za-z0-9_]+|CONFIG_SHELL|CC|LD|NM|AR|OBJCOPY|OBJDUMP|READELF|STRIP|PAHOLE|RESOLVE_BTFIDS|SRCARCH|ARCH|srctree|objtree|LINUXINCLUDE|NOSTDINC_FLAGS|LDFLAGS_vmlinux|CFLAGS_vmlinux|KALLSYMS[A-Za-z0-9_]*|MAKE|HOSTCC)=" > .kbuild-unit-link-env; } 2>/dev/null || :' scripts/link-vmlinux.sh
          make ${configTarget}
          runHook postConfigure
        '';
        buildPhase = ''
          runHook preBuild
          make -j"$NIX_BUILD_CORES" vmlinux
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out
          # Monolithic reference for the byte-identity gate below.
          cp vmlinux $out/vmlinux
          nix-kbuild-unit harvest \
            --objtree . \
            --srctree ${srcTree} \
            --generated-out $out/generated \
            > $out/plan.json
          runHook postInstall
        '';
      }
      // lib.optionalAttrs contentAddressed contentAddressing);

    unitsNix = pkgs.runCommand "kbuild-units.nix" {
      nativeBuildInputs = [nixKbuildUnit];
    } ''
      nix-kbuild-unit render ${lib.optionalString contentAddressed "--content-addressed"} \
        < ${plan}/plan.json > $out
    '';

    imported = import unitsNix {
      inherit pkgs;
      src = srcTree;
      generated = "${plan}/generated";
      extraNativeBuildInputs = kbuildInputs;
      extraEnv = reproEnv;
    };

    # The exit gate: the unit-composed vmlinux must be byte-identical to the
    # monolithic vmlinux from the very kbuild the plan was harvested from.
    vmlinuxEquivalence = pkgs.runCommand "kbuild-unit-vmlinux-equivalence" {} ''
      cmp ${imported.vmlinux}/vmlinux ${plan}/vmlinux
      sha256sum ${imported.vmlinux}/vmlinux ${plan}/vmlinux > $out
    '';
  in
    imported
    // {
      inherit plan unitsNix srcTree vmlinuxEquivalence;
    };
in {
  inherit buildKernel;
}
