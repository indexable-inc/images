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

Each unit's build tree is scoped to the sources its .cmd recorded (#3412):
compile units see their .c plus tracked headers as per-file store paths,
the link unit adds the script-read Makefile and header trees, and archive
aggregation sees only dep unit outputs plus the snapshot. A one-file body
edit thus re-instantiates one TU derivation and its link chain; a
comment-only edit cuts off at the unchanged CA object.
*/
{
  lib,
  # astlog-ignore: no-pkgs-in-callpackage
  pkgs,
  nixKbuildUnit,
}: let
  # Toolset the plan kbuild needs; unit replays get the same set so a saved
  # command never resolves a tool differently than the plan build did.
  kbuildInputs = [
    pkgs.bison
    pkgs.flex
    pkgs.bc
    pkgs.perl
    pkgs.python3
    pkgs.openssl
    pkgs.elfutils
    pkgs.zlib
    pkgs.pahole
    pkgs.kmod
    pkgs.cpio
  ];

  # Reproducibility pins. #3410 (repro baseline) has no verdict yet, so this
  # uses a fixed epoch timestamp string rather than the empty-string form;
  # reconcile when the baseline lands.
  reproEnv = {
    KBUILD_BUILD_TIMESTAMP = "Thu Jan  1 00:00:00 UTC 1970";
    KBUILD_BUILD_USER = "nixbld";
    KBUILD_BUILD_HOST = "ix";
    KBUILD_BUILD_VERSION = "1";
    # stdenv setup's _addRpathPrefix otherwise prepends `-rpath $out/lib` to
    # NIX_LDFLAGS; the 32-bit vdso is linked as a .so, so the builder's own
    # store path lands inside vmlinux and plan/unit outputs can never match
    # (#3411: gate diverged by exactly this 30-byte hash). NIX_DONT_SET_RPATH
    # is the wrong knob: it governs the ld-wrapper's -L auto-rpath and is
    # suffix-salted, so the bare name is ignored.
    NIX_NO_SELF_RPATH = "1";
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
        # The sed below inserts a brace group after line 1; substituteInPlace
        # only replaces, and the script has no stable anchor string to
        # replace, while sed's `1a` address cannot stop matching.
        # astlog-ignore: prefer-substituteinplace
        configurePhase = ''
          # shell
          runHook preConfigure
          # The cc-wrapper seeds -frandom-seed from a fragment of this
          # derivation's $out and appends NIX_CFLAGS_COMPILE after user argv,
          # so the last seed wins codegen. Appending a constant pins objects,
          # but the assembler listings the snapshot keeps (asm-offsets.s,
          # bounds.s) record every passed option, so a surviving $out-derived
          # seed would shift the snapshot's content address on every plan drv
          # change and re-execute every unit. Strip the wrapper seed and pin
          # the constant as the only seed, here and in every unit replay
          # (templates/units.nix.in).
          NIX_CFLAGS_COMPILE=$(printf '%s' "''${NIX_CFLAGS_COMPILE:-}" | sed 's/-frandom-seed=[^ ]*//g')
          export NIX_CFLAGS_COMPILE="$NIX_CFLAGS_COMPILE -frandom-seed=kbuild-unit"
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
          # shell
          runHook preBuild
          make -j"$NIX_BUILD_CORES" vmlinux
          runHook postBuild
        '';
        installPhase = ''
          # shell
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

    unitsNix =
      pkgs.runCommand "kbuild-units.nix" {
        nativeBuildInputs = [nixKbuildUnit];
      } ''
        nix-kbuild-unit render ${lib.optionalString contentAddressed "--content-addressed"} \
          < ${plan}/plan.json > $out
      '';

    # Stable-path hop for the snapshot: the plan's output path shifts with
    # every source edit (its reference vmlinux changes), which would perturb
    # every unit's `generated` input. Re-homing the snapshot in its own CA
    # derivation keeps the path fixed while the harvested content is
    # unchanged, so unrelated units stay cached across plan reruns (#3412).
    generatedSnapshot =
      pkgs.runCommand "kbuild-unit-generated"
      (lib.optionalAttrs contentAddressed contentAddressing) ''
        cp -a ${plan}/generated $out
      '';

    imported = import unitsNix {
      inherit pkgs;
      src = srcTree;
      generated = "${generatedSnapshot}";
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
