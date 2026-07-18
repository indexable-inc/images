/**
kbuild-unit: per-translation-unit content-addressed Linux kernel builds
(#3411), the kbuild analog of lib/rust/cargo-unit.nix.

Stage 1 (`plan`) runs a full monolithic kbuild under pinned reproducibility
env, then harvests every `.cmd` file plus a snapshot of build-created
non-unit files (kconfig output, generated headers, host tools, linker
scripts) into `plan.json`. Stage 2 renders plan.json to units.nix (IFD),
which builds one CA derivation per unit by replaying its exact saved
command inside a symlink farm of src + snapshot + dep unit outputs.

What the plan kbuild compiles is governed by `planStrategy` (#3413):

- "skeleton" (default): the plan consumes a directives-only reduction of the
  source tree (`nix-kbuild-unit skeleton`, a CA derivation) and a gcc-named
  PATH shim substitutes stub objects for every reduced TU while recording
  real dep sets via `-E` + the argv's own `-Wp,-MMD`. Function-body edits
  reduce to a byte-identical skeleton, so the plan's resolved derivation is
  already realised and never reruns; only the edited TU's unit (plus its
  link chain) rebuilds. The plan's vmlinux is stub garbage, so equivalence
  gates compare against `referenceKernel`, a real monolithic build.
- "ccache": the plan consumes the real tree and the shim runs the compiler
  under ccache against a host-mounted cache dir (see
  services.ci-runner.kbuildCcache). The static fallback lane for configs
  whose plan-time tooling rejects stub objects; outputs are bit-identical
  with or without cache hits, ccache only changes wall time.
- "full": exact pre-#3413 behavior (plan = reference), kept as the
  debugging baseline.

On CONFIG_MODULES=y configs the plan also runs `make modules`, and the unit
graph extends over module objects, both modpost passes (`vmlinux.symvers`
with its generated ksymtab source, `Module.symvers` with each module's
`.mod.c`), and every final `.ko` link; `modulesEquivalence` byte-compares
each unit-built module against the reference build's (#3413).

Each unit's build tree is scoped to the sources its .cmd recorded (#3412):
compile units see their .c plus tracked headers as per-file store paths,
the link unit adds the script-read Makefile and header trees, and archive
aggregation sees only dep unit outputs plus the snapshot. Units always
compile the real sources regardless of `planStrategy`; the skeleton exists
only to make the plan's inputs body-independent.
*/
{
  lib,
  # astlog-ignore: no-pkgs-in-callpackage
  pkgs,
  nixKbuildUnit,
  writeBashApplication,
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

  # The cc-wrapper seeds -frandom-seed from a fragment of the building
  # derivation's $out and appends NIX_CFLAGS_COMPILE after user argv, so the
  # last seed wins codegen. Appending a constant pins objects, but the
  # assembler listings the snapshot keeps (asm-offsets.s, bounds.s) record
  # every passed option, so a surviving $out-derived seed would shift the
  # snapshot's content address on every plan drv change and re-execute every
  # unit. Strip the wrapper seed and pin the constant as the only seed --
  # identically in the plan, the reference build, and every unit replay
  # (templates/units.nix.in), or their objects could never match.
  pinRandomSeed = ''
    # shell
    NIX_CFLAGS_COMPILE=$(printf '%s' "''${NIX_CFLAGS_COMPILE:-}" | sed 's/-frandom-seed=[^ ]*//g')
    export NIX_CFLAGS_COMPILE="$NIX_CFLAGS_COMPILE -frandom-seed=kbuild-unit"
  '';

  buildVmlinuxAndModules = ''
    # shell
    runHook preBuild
    make -j"$NIX_BUILD_CORES" vmlinux
    # The modules pass (modpost over modules.order, per-module .mod.o
    # and .ko links) exists only on CONFIG_MODULES=y configs; tinyconfig
    # has none, and the module-less plan must stay byte-identical so
    # every existing unit realisation cuts off.
    if grep -q '^CONFIG_MODULES=y$' .config; then
      make -j"$NIX_BUILD_CORES" modules
    fi
    runHook postBuild
  '';

  # Reference copies for the byte-identity gates: the monolithic vmlinux,
  # every .ko at its objtree-relative path, and the modpost symvers dump.
  installReference = ''
    # shell
    cp vmlinux $out/vmlinux
    if grep -q '^CONFIG_MODULES=y$' .config; then
      mkdir -p $out/reference-modules
      find . -name '*.ko' -exec cp --parents {} $out/reference-modules \;
      cp Module.symvers $out/reference-modules/Module.symvers
    fi
  '';

  # The plan-only compiler shim (#3413), named literally `gcc` so kbuild's
  # $(CC) resolves to it via PATH while savedcmd_* keeps recording plain
  # `gcc`; unit replays then resolve the real cc-wrapper. Skeleton mode stubs
  # reduced TUs' objects (marker-gated); ccache mode wraps the real compiler
  # in ccache. See the script for the full contract.
  planCcShim = writeBashApplication {
    name = "gcc";
    text = builtins.readFile ./kbuild-plan-cc.sh;
  };

  # Skeleton-only linker shim: `--defsym`s the linker-script-referenced
  # symbols the stub objects no longer define (plus lenient resolution),
  # gated to vmlinux-ish outputs so vdso/realmode links stay byte-exact.
  # See the script for the full contract.
  planLdShim = writeBashApplication {
    name = "ld";
    text = builtins.readFile ./kbuild-plan-ld.sh;
  };

  # Appended by the ld shim to the stub vmlinux links: minimal one-entry
  # stand-ins for sections the post-link tools require to exist (sorttable
  # hard-fails on a vmlinux with no __ex_table; x86 entries are 3 ints on
  # both widths). Assembled for each ELF class the shim may link
  # (x86 tinyconfig is a 32-bit kernel, defconfig 64-bit).
  planLdPlaceholders = pkgs.runCommand "kbuild-unit-ld-placeholders" {} ''
    mkdir -p $out
    # sorttable also patches main_extable_sort_needed (kernel/extable.c in
    # real builds) through the symtab, so it needs backing storage here, not
    # a --defsym absolute.
    printf '%s\n' \
      '.section __ex_table,"a"' \
      '.balign 4' \
      '.long 0, 0, 0' \
      '.section .init.data,"aw"' \
      '.globl main_extable_sort_needed' \
      '.type main_extable_sort_needed, @object' \
      '.size main_extable_sort_needed, 4' \
      '.balign 4' \
      'main_extable_sort_needed:' \
      '.long 1' \
      > extable.s
    ${pkgs.stdenv.cc.bintools.bintools}/bin/as --64 -o $out/placeholder-64.o extable.s
    ${pkgs.stdenv.cc.bintools.bintools}/bin/as --32 -o $out/placeholder-32.o extable.s
  '';

  buildKernel = {
    src,
    configTarget ? "tinyconfig",
    contentAddressed ? true,
    planStrategy ? "skeleton",
    skeletonKeep ? [],
  }: let
    # Extra env for the plan derivation, keyed by strategy; the lookup is
    # also the planStrategy enum check. Env is invisible to savedcmd_*, so
    # none of this leaks into replayed commands.
    planShimEnv =
      {
        skeleton = {
          KBUILD_UNIT_CC_MODE = "skeleton";
          KBUILD_UNIT_CC_REAL = "${pkgs.stdenv.cc}/bin/gcc";
          KBUILD_UNIT_LD_REAL = "${pkgs.stdenv.cc.bintools}/bin/ld";
          KBUILD_UNIT_LD_PLACEHOLDER_DIR = "${planLdPlaceholders}";
        };
        ccache = {
          KBUILD_UNIT_CC_MODE = "ccache";
          KBUILD_UNIT_CC_REAL = "${pkgs.stdenv.cc}/bin/gcc";
          KBUILD_UNIT_CCACHE = lib.getExe pkgs.ccache;
          # The cache dir a builder host opts into mounting via
          # services.ci-runner.kbuildCcache; the shim warns and builds
          # uncached when it is absent. Size cap and cwd/time hygiene ride
          # here so every writer applies the same policy.
          CCACHE_DIR = "/var/cache/kbuild-ccache";
          CCACHE_MAXSIZE = "30G";
          CCACHE_NOHASHDIR = "1";
          CCACHE_SLOPPINESS = "time_macros";
        };
        full = {};
      }
      .${
        planStrategy
      }
        or (throw "kbuild-unit: unknown planStrategy \"${planStrategy}\" (expected \"skeleton\", \"ccache\", or \"full\")");

    # The kernel `src` is a tarball; units and harvest need the unpacked tree
    # as a directory (symlink-farm base and byte-compare reference).
    srcTree = pkgs.srcOnly {
      name = "kbuild-unit-src";
      inherit src;
      # Unpack only; omitting stdenv trips this repo's abort-on-warn.
      stdenv = pkgs.stdenvNoCC;
    };

    # Directives-only reduction of the tree (#3413). CA: a function-body edit
    # rebuilds this derivation but lands the identical output path, so the
    # plan's resolved derivation is already realised and does not rerun. A
    # header/Makefile/Kconfig edit changes the skeleton and reruns the plan.
    skeletonTree =
      pkgs.runCommand "kbuild-unit-skeleton"
      ({nativeBuildInputs = [nixKbuildUnit];} // lib.optionalAttrs contentAddressed contentAddressing)
      ''
        nix-kbuild-unit skeleton --src ${srcTree} --out $out \
          ${lib.concatMapStringsSep " " (glob: "--keep ${lib.escapeShellArg glob}") skeletonKeep}
      '';

    planSrc =
      if planStrategy == "skeleton"
      then skeletonTree
      else srcTree;

    plan = pkgs.stdenv.mkDerivation ({
        pname = "kbuild-unit-plan";
        version = configTarget;
        src = planSrc;
        nativeBuildInputs = kbuildInputs ++ [nixKbuildUnit];
        env = reproEnv // planShimEnv;
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
          ${lib.optionalString (planStrategy != "full") ''
            # The shim must shadow the cc-wrapper for $(CC) = gcc; PATH is
            # env, so savedcmd_* still records bare `gcc`.
            export PATH=${planCcShim}/bin:$PATH
          ''}
          ${lib.optionalString (planStrategy == "skeleton") ''
            export PATH=${planLdShim}/bin:$PATH
          ''}
          ${lib.optionalString (planStrategy == "ccache") ''
            export CCACHE_BASEDIR="$PWD"
          ''}
          ${pinRandomSeed}
          # Capture the env kbuild exports to scripts/link-vmlinux.sh so the
          # rendered link unit can replay it (CC/LD/LINUXINCLUDE/KBUILD_* for
          # the in-script init/version-timestamp.o compile and postlink make).
          # The dump is build-created and not unit-owned, so harvest sweeps it
          # into the generated snapshot on its own. `export -p` because make
          # runs the script under $(CONFIG_SHELL). The KBUILD_UNIT_* shim
          # vars match the KBUILD_ prefix but must stay out: they carry
          # plan-only store paths, and a snapshot that shifts with shim
          # tweaks would re-execute every unit.
          # `|| :` and the muted stderr keep unit replays quiet: there the
          # dump target is a store symlink from the generated snapshot, the
          # rewrite fails by design, and the sourced snapshot env is already
          # identical to what the dump would produce.
          sed -i '1a { export -p | grep -E "^(declare -x |export )(KBUILD_[A-Za-z0-9_]+|CONFIG_SHELL|CC|LD|NM|AR|OBJCOPY|OBJDUMP|READELF|STRIP|PAHOLE|RESOLVE_BTFIDS|SRCARCH|ARCH|srctree|objtree|LINUXINCLUDE|NOSTDINC_FLAGS|LDFLAGS_vmlinux|CFLAGS_vmlinux|KALLSYMS[A-Za-z0-9_]*|MAKE|HOSTCC)=" | grep -vE "^(declare -x |export )KBUILD_UNIT_" > .kbuild-unit-link-env; } 2>/dev/null || :' scripts/link-vmlinux.sh
          make ${configTarget}
          runHook postConfigure
        '';
        buildPhase = buildVmlinuxAndModules;
        installPhase = ''
          # shell
          runHook preInstall
          mkdir -p $out
          ${
            # Under skeleton the plan's vmlinux and .ko files are stub garbage;
            # the gates compare against referenceKernel instead.
            lib.optionalString (planStrategy != "skeleton") installReference
          }
          nix-kbuild-unit harvest \
            --objtree . \
            --srctree ${planSrc} \
            --generated-out $out/generated \
            > $out/plan.json
          runHook postInstall
        '';
      }
      // lib.optionalAttrs contentAddressed contentAddressing);

    # The real monolithic build the equivalence gates compare against when
    # the plan's own outputs are stubs (skeleton strategy). Same pins as the
    # plan, no harvest, no shim: this is the P2-era plan build minus
    # plan.json.
    referenceKernel = pkgs.stdenv.mkDerivation ({
        pname = "kbuild-unit-reference";
        version = configTarget;
        src = srcTree;
        nativeBuildInputs = kbuildInputs;
        env = reproEnv;
        dontFixup = true;
        configurePhase = ''
          # shell
          runHook preConfigure
          ${pinRandomSeed}
          make ${configTarget}
          runHook postConfigure
        '';
        buildPhase = buildVmlinuxAndModules;
        installPhase = ''
          # shell
          runHook preInstall
          mkdir -p $out
          ${installReference}
          runHook postInstall
        '';
      }
      // lib.optionalAttrs contentAddressed contentAddressing);

    reference =
      if planStrategy == "skeleton"
      then referenceKernel
      else plan;

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
    # monolithic vmlinux from the reference build (the plan build itself for
    # ccache/full, the dedicated referenceKernel for skeleton).
    vmlinuxEquivalence = pkgs.runCommand "kbuild-unit-vmlinux-equivalence" {} ''
      cmp ${imported.vmlinux}/vmlinux ${reference}/vmlinux
      sha256sum ${imported.vmlinux}/vmlinux ${reference}/vmlinux > $out
    '';

    # Module exit gate (#3413): every unit-built .ko and the modules-pass
    # symvers dump must be byte-identical to the reference build's. On a
    # module-less config the gate instead asserts the reference build agreed
    # that there was nothing to compare.
    modulesEquivalence =
      if imported.moduleSymvers == null
      then
        pkgs.runCommand "kbuild-unit-modules-equivalence" {} ''
          test ! -e ${reference}/reference-modules
          echo "no modules (CONFIG_MODULES=n)" > $out
        ''
      else
        pkgs.runCommand "kbuild-unit-modules-equivalence" {} ''
          cd ${reference}/reference-modules
          # Same module set on both sides, then byte-compare each member.
          diff <(find . -name '*.ko' | sort) \
            <(cd ${imported.modules} && find . -name '*.ko' | sort)
          count=0
          while IFS= read -r -d "" ref; do
            cmp "$ref" ${imported.modules}/"$ref"
            count=$((count + 1))
          done < <(find . -name '*.ko' -print0)
          cmp Module.symvers ${imported.moduleSymvers}/Module.symvers
          echo "$count modules byte-identical" > $out
        '';
  in
    imported
    // {
      inherit plan unitsNix srcTree skeletonTree referenceKernel vmlinuxEquivalence modulesEquivalence;
    };
in {
  inherit buildKernel;
}
