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

The skeleton reduction is sharded per top-level source directory (drivers/
one level deeper), each shard a CA derivation over an eval-time
`builtins.path` slice of just that directory, composed back into one tree
whose bytes -- and therefore store path -- match a whole-tree reduction
(#3706). A body edit re-reduces exactly the directory it touched; every
other shard, the composed skeleton, and the plan behind it resolve to
already-realised outputs. The same slicing serves the pre-unpacked
`srcTree` argument (a developer working tree as a plain path, under
--impure eval), which keeps whole-tree store ingestion out of the edit
loop entirely: builtins.path re-ingests only the directory an edit
touched, and the per-file source farm in units.nix re-keys only the
edited translation units.

On CONFIG_MODULES=y configs the plan also runs `make modules`, and the unit
graph extends over module objects, both modpost passes (`vmlinux.symvers`
with its generated ksymtab source, `Module.symvers` with each module's
`.mod.c`), and every final `.ko` link; `modulesEquivalence` byte-compares
each unit-built module against the reference build's (#3413).

Sharing over the fleet cache rides `cachePushRoot` (#3413): one linkFarm
whose runtime closure spans every unit output plus the IFD artifacts
(plan, rendered units.nix, snapshot, skeleton). Building that root on a
CI dispatcher enqueues ONE obligation whose recursive closure the cache
drainer publishes -- NARs and CA realisations -- so other hosts
substitute units instead of rebuilding, while the mass unit build itself
runs with the per-derivation post-build hook disabled (one hook enqueue
per unit serialized 3.6k-unit builds to a crawl under queue
backpressure; see #3413).

Plan reruns must reproduce the snapshot bit-identically or every unit
re-executes at defconfig scale, so the plan pins down every observed
nondeterminism carrier: the build tree always unpacks to a fixed
directory name (DWARF comp_dir and the vdso build-id record the absolute
build path; a name that shifted with the src derivation made the
embedded vdso diverge from the reference's), snapshotted host tools
under tools/ are stripped of debug info (tools/build compiles with -g,
and objtool's .debug_line_str differed across otherwise identical plan
builds), and the link-env dump is emitted by one bash helper with sorted
names and printf %q quoting instead of whichever shell CONFIG_SHELL
resolves to (its `export -p` quoting style flipped between runs).

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

  # DWARF comp_dir, the vdso build-id, and objtool's .debug_line_str all
  # record the absolute build tree path, and parts of that DWARF flow into
  # unit inputs (the vdso .so embedded via the generated vdso-image-*.c) and
  # the snapshot (host tools, vdso .so.dbg). Unpack to one fixed name --
  # matching the `kbuild-tree` unit replays build in -- so those bytes agree
  # between the plan (whose src name varies by strategy: skeletonTree vs
  # srcTree), the reference build, and every plan rerun (#3413).
  pinBuildTreeName = ''
    # shell
    mv "$sourceRoot" kbuild-tree
    sourceRoot=kbuild-tree
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

  # Deterministic emitter for the `.kbuild-unit-link-env` dump. The dump used
  # to be `export -p` output, whose quoting style belongs to whichever shell
  # CONFIG_SHELL resolves to and flipped across plan builds (#3413 PR A),
  # shifting the snapshot CA and re-executing every unit. One bash helper
  # with LC_ALL=C-sorted names and printf %q quoting owns the format instead.
  # Inherited env is the interface: link-vmlinux.sh exports what its
  # subprocesses need, so the helper reads its own environment. KBUILD_UNIT_*
  # stays out: those carry plan-only shim store paths, and a snapshot that
  # shifts with shim tweaks would re-execute every unit.
  linkEnvDump = writeBashApplication {
    name = "kbuild-unit-link-env-dump";
    runtimeInputs = [pkgs.coreutils];
    text = ''
      # shell
      # Prefix expansion, not compgen: writeBashApplication's bash builds
      # without programmable completion. Every variable in this fresh
      # process came from the environ, so "is set" is "is exported".
      names=()
      for name in "''${!KBUILD_@}" "''${!KALLSYMS@}" \
        CONFIG_SHELL CC LD NM AR OBJCOPY OBJDUMP READELF STRIP PAHOLE \
        RESOLVE_BTFIDS SRCARCH ARCH srctree objtree LINUXINCLUDE \
        NOSTDINC_FLAGS LDFLAGS_vmlinux CFLAGS_vmlinux MAKE HOSTCC; do
        [ -n "''${!name+x}" ] || continue
        case $name in KBUILD_UNIT_*) continue ;; esac
        names+=("$name")
      done
      while IFS= read -r name; do
        printf 'export %s=%q\n' "$name" "''${!name}"
      done < <(printf '%s\n' "''${names[@]}" | LC_ALL=C sort -u)
    '';
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
    # SRSO stand-in (see kbuild-plan-ld.sh): the alias pair placed exactly
    # like arch/x86/lib/retpoline.S places it, so the linker script's
    # `. = srso_alias_untrain_ret | 0x104104` moves the location counter
    # forward and the pair's alias ASSERT (XOR == 0x104104) holds. The 2MiB
    # alignment keeps the untrain symbol's low bits clear of the OR mask.
    printf '%s\n' \
      '.section .text..__x86.rethunk_untrain,"ax"' \
      '.balign 2097152' \
      '.globl srso_alias_untrain_ret' \
      '.type srso_alias_untrain_ret, @function' \
      'srso_alias_untrain_ret:' \
      '.byte 0xc3' \
      '.section .text..__x86.rethunk_safe,"ax"' \
      '.globl srso_alias_safe_ret' \
      '.type srso_alias_safe_ret, @function' \
      'srso_alias_safe_ret:' \
      '.byte 0xc3' \
      > srso.s
    ${pkgs.stdenv.cc.bintools.bintools}/bin/as --64 -o $out/placeholder-srso-64.o srso.s
  '';

  buildKernel = {
    # The kernel source tarball (pkgs.linux_*.src), unpacked once below.
    # Exactly one of `src`/`srcTree` must be given.
    src ? null,
    # A pre-unpacked kernel tree instead: a derivation output (say
    # applyPatches over a checkout) or a plain path (a developer working
    # tree, which needs --impure eval). Slicing reads straight from the
    # tree, so a plain-path edit loop never pays a whole-tree store copy
    # (#3706).
    srcTree ? null,
    # Pre-ingested per-directory slices instead of a tree: an attrset from
    # slice key to store path, with the same key shape the automatic
    # slicing produces ("" for the files directly at the tree root, one key
    # per top-level directory, and "<dir>/<sub>" plus a files-only "<dir>"
    # for deep-sharded directories such as drivers/). This scopes the
    # driver-side cost of an edit loop to the directory an edit touched
    # (`nix store add` it and re-evaluate); note that current Nix still
    # re-ingests eval-time sources once per evaluation -- builtins.path has
    # no cross-eval memoization -- which is the measured residual of the
    # #3706 loop. Needs --impure eval, like a plain-path srcTree: pure
    # evaluation refuses to read store paths it was not handed context
    # for. Outputs stay fully content-addressed either way.
    srcSlices ? null,
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
    # as a directory (symlink-farm base and byte-compare reference). A caller
    # that already has the tree passes `srcTree` and skips the unpack.
    tree =
      if builtins.length (lib.filter (given: given != null) [src srcTree srcSlices]) != 1
      then throw "kbuild-unit: pass exactly one of src (tarball), srcTree (unpacked tree), or srcSlices (pre-ingested slices)"
      else if srcTree != null
      then srcTree
      else if src != null
      then
        pkgs.srcOnly {
          name = "kbuild-unit-src";
          inherit src;
          # Unpack only; omitting stdenv trips this repo's abort-on-warn.
          stdenv = pkgs.stdenvNoCC;
        }
      else throw "kbuild-unit: srcSlices given; there is no whole tree to read";

    # A plain-path tree (developer working tree) is read in place; a
    # derivation tree is realised once and read from its output (the readDir
    # below is the eval/build boundary: instantiating the plan now needs the
    # tree built first, which was already true in practice via the source
    # farm in units.nix).
    treeIsPath = builtins.isPath tree;
    treeRoot =
      if treeIsPath
      then tree
      else "${tree}";
    treeSubPath = rel:
      if treeIsPath
      then tree + ("/" + rel)
      else "${tree}/${rel}";

    # `.git` never enters the slices: an unpacked tarball has none, and a
    # working tree must slice identically to the tarball it came from
    # (setlocalversion must also never see it, or the plan would grow a
    # local-version suffix the reference build lacks). It is a directory in
    # a checkout but a file in a linked worktree, so both shapes go.
    treeEntries = builtins.readDir treeRoot;
    treeDirNames =
      lib.filter
      (name: treeEntries.${name} == "directory" && name != ".git")
      (builtins.attrNames treeEntries);
    subDirNames = dir: let
      entries = builtins.readDir (treeSubPath dir);
    in
      lib.filter (name: entries.${name} == "directory") (builtins.attrNames entries);

    # Per-directory slices (#3706): each top-level directory (and each
    # drivers/ subdirectory) becomes its own content-addressed store path,
    # so an edit re-ingests exactly the directory it touched while every
    # other slice keeps its store path. Everything downstream keys on the
    # slices -- the skeleton shards, the per-file source farm, and the link
    # unit's directory scopes (units.nix resolves srctree paths through
    # them) -- which is what lets Nix's source-ingestion memoization hold
    # across evals for everything an edit did not touch: builtins.path
    # re-hashing is only cheap when its source sits inside an unchanged,
    # valid store path.
    #
    # Slices are always eval-ingested source paths, never derivation
    # outputs: builtins.path cannot consume a content-addressed derivation
    # output (its eval-time string is an unresolved placeholder), so a
    # `src`/`srcTree` tree pays one whole-tree hash per edit here. An edit
    # loop that wants to pay only for the directory it touched pre-ingests
    # its slices and passes `srcSlices` instead.
    sanitizeRel = rel: lib.replaceStrings ["/"] ["-"] rel;
    sliceName = key: merge: "kbuild-src-slice-${
      if key == ""
      then "root-files"
      else sanitizeRel key + lib.optionalString merge "-files"
    }";
    dirSlice = name: rel:
      builtins.path {
        inherit name;
        path = treeSubPath rel;
      };
    # Files directly inside `subPath` (the tree root, or a deep-sharded
    # directory): rejecting directories in the filter also stops the walk,
    # so this touches one level only.
    filesOnlySlice = name: subPath:
      builtins.path {
        inherit name;
        path =
          if subPath == ""
          then treeRoot
          else treeSubPath subPath;
        filter = path: type: type != "directory" && baseNameOf path != ".git";
      };

    # Directories sharded one level deeper: drivers/ alone is well over half
    # of the tree, so a top-level shard would still re-reduce all of it on
    # any driver edit.
    deepSharded = ["drivers"];

    # One slice (and skeleton shard) per spec: `dest` is both the slice key
    # units.nix resolves srctree paths through and where the shard's output
    # lands in the composed tree; `merge` marks a files-only spec (content
    # merges into an existing directory, and the slice serves the files
    # directly inside that directory) as opposed to a directory spec (the
    # slice *is* the directory).
    shardSpecs =
      if srcSlices != null
      then let
        keys = builtins.attrNames srcSlices;
      in
        map (
          key: let
            # A key with children (or the root) is a files-only slice; the
            # rest are whole directories. Same shape the automatic slicing
            # produces below.
            merge = key == "" || lib.any (other: lib.hasPrefix "${key}/" other) keys;
          in {
            dest = key;
            inherit merge;
            # Re-ingesting normalizes the given path (string or path) into
            # a tracked source path; when it already sits in the store and
            # is unchanged, this memoizes instead of re-hashing.
            slice = builtins.path {
              name = sliceName key merge;
              path = srcSlices.${key};
            };
          }
        )
        keys
      else autoShardSpecs;

    autoShardSpecs =
      [
        {
          dest = "";
          merge = true;
          slice = filesOnlySlice (sliceName "" true) "";
        }
      ]
      ++ lib.concatMap (
        dir:
          if lib.elem dir deepSharded
          then
            [
              {
                dest = dir;
                merge = true;
                slice = filesOnlySlice (sliceName dir true) dir;
              }
            ]
            ++ map (sub: {
              dest = "${dir}/${sub}";
              merge = false;
              slice = dirSlice (sliceName "${dir}/${sub}" false) "${dir}/${sub}";
            }) (subDirNames dir)
          else [
            {
              dest = dir;
              merge = false;
              slice = dirSlice (sliceName dir false) dir;
            }
          ]
      )
      treeDirNames;

    # Directives-only reduction of one slice (#3413, sharded per #3706).
    # `--prefix` re-roots classification so the keep-allowlist and the
    # scripts//tools/ verbatim rules keep matching tree-relative paths. CA:
    # a body edit rebuilds exactly its own shard, which lands the identical
    # output path, so the compose below (and the plan behind it) resolves to
    # already-realised outputs and never reruns. A header/Makefile/Kconfig
    # edit changes its shard's output and legitimately reruns the plan.
    shardFor = {
      dest,
      merge,
      slice,
    }:
      pkgs.runCommand "kbuild-unit-skeleton-shard${lib.optionalString (dest != "") "-${sanitizeRel dest}"}${lib.optionalString (merge && dest != "") "-files"}"
      ({nativeBuildInputs = [nixKbuildUnit];} // lib.optionalAttrs contentAddressed contentAddressing)
      ''
        nix-kbuild-unit skeleton --src ${slice} --out $out \
          --prefix ${lib.escapeShellArg dest} \
          ${lib.concatMapStringsSep " " (glob: "--keep ${lib.escapeShellArg glob}") skeletonKeep}
      '';

    shards = map (spec: spec // {drv = shardFor spec;}) shardSpecs;

    # Compose per-spec pieces back into one tree at each spec's dest;
    # `getPath` selects the piece (a shard's reduced output for the
    # skeleton, the raw slice for the whole-tree compose).
    composeTree = name: specs: getPath:
      pkgs.runCommand name
      (lib.optionalAttrs contentAddressed contentAddressing)
      ''
        mkdir -p $out
        ${lib.concatMapStrings (
            spec: let
              dest =
                if spec.dest == ""
                then "$out"
                else "$out/${lib.escapeShellArg spec.dest}";
              parent = builtins.dirOf spec.dest;
            in
              if spec.merge
              then ''
                mkdir -p ${dest}
                cp -a ${getPath spec}/. ${dest}/
                # cp -a src/. propagates the store piece's read-only mode
                # onto the target directory; later pieces still copy into
                # it. NAR serialization carries no directory modes, so this
                # cannot perturb the content address.
                chmod u+w ${dest}
              ''
              else ''
                ${lib.optionalString (parent != ".") "mkdir -p $out/${lib.escapeShellArg parent}\n"}cp -a ${getPath spec} ${dest}
              ''
          )
          specs}
      '';

    # The composed skeleton, byte-identical (same CA store path) to the
    # previous single-derivation whole-tree reduction, so the plan contract
    # does not move. The compose only re-runs when a shard's *output*
    # changes; after a body edit every shard resolves to its existing
    # realisation and the compose never executes.
    skeletonTree = composeTree "kbuild-unit-skeleton" shards (shard: shard.drv);

    # Every shard in one farm: probes build it to warm exactly the sharded
    # reduction, and the cache lane pushes it so other hosts substitute
    # shard realisations instead of re-reducing.
    skeletonShards = pkgs.linkFarm "kbuild-unit-skeleton-shards" (map (shard: {
        name =
          if shard.dest == ""
          then "root-files"
          else sanitizeRel shard.dest + lib.optionalString shard.merge "-files";
        path = shard.drv;
      })
      shards);

    # A derivation-consumable whole-tree path. For a plain-path tree the
    # copy happens lazily at instantiation -- the equivalence gates and the
    # non-skeleton plan strategies force it, the skeleton edit loop never
    # does -- and `.git` stays out for the same reason it stays out of the
    # slices.
    wholeTree =
      if srcSlices != null
      then
        # No whole tree was given; compose one from the slices (content-
        # identical to the tree they came from). Only the equivalence
        # gates' reference build and the non-skeleton plan strategies force
        # this.
        composeTree "kbuild-unit-src" shardSpecs (spec: spec.slice)
      else if treeIsPath
      then
        builtins.path {
          name = "kbuild-unit-src";
          path = tree;
          filter = path: _type: baseNameOf path != ".git";
        }
      else tree;

    planSrc =
      if planStrategy == "skeleton"
      then skeletonTree
      else wholeTree;

    plan = pkgs.stdenv.mkDerivation ({
        pname = "kbuild-unit-plan";
        version = configTarget;
        src = planSrc;
        nativeBuildInputs = kbuildInputs ++ [nixKbuildUnit linkEnvDump];
        env = reproEnv // planShimEnv;
        # Unit replays must see identical bits: no fixup strip / patchelf over
        # the reference vmlinux or the snapshot (the targeted host-tool strip
        # in installPhase is deliberate; fixup would also rewrite outputs the
        # gates byte-compare).
        dontFixup = true;
        postUnpack = pinBuildTreeName;
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
          # into the generated snapshot on its own. The helper (see
          # linkEnvDump above) owns the selection and the canonical quoting;
          # it is called by bare name so the patched script -- itself a
          # snapshot member -- carries no store path.
          # `|| :` and the muted stderr keep unit replays quiet: there the
          # helper is off PATH and the dump target is a store symlink from
          # the generated snapshot, the redump fails by design, and the
          # sourced snapshot env is already identical to what the dump would
          # produce.
          sed -i '1a { kbuild-unit-link-env-dump > .kbuild-unit-link-env; } 2>/dev/null || :' scripts/link-vmlinux.sh
          make ${configTarget}
          runHook postConfigure
        '';
        buildPhase = buildVmlinuxAndModules;
        installPhase = ''
          # shell
          runHook preInstall
          # The link-env dump is written mid-link by the helper call patched
          # into link-vmlinux.sh at configure time, whose failures are muted
          # for unit replays; catch a plan-side regression loudly before an
          # empty dump ships in the snapshot and breaks the link unit's
          # replay.
          test -s .kbuild-unit-link-env
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
          # tools/build compiles host tools with -g, and their DWARF varies
          # with the build path when the sandbox is off (#3413 PR A: objtool
          # differed only in .debug_line_str across identical-source plan
          # builds, re-executing every unit). The snapshot only needs the
          # tools to run, so drop the debug sections deterministically.
          if [ -d $out/generated/tools ]; then
            find $out/generated/tools -type f | while IFS= read -r tool; do
              if [ "$(head -c4 "$tool" | od -An -tx1 | tr -d ' ')" = 7f454c46 ]; then
                strip --strip-debug "$tool"
              fi
            done
          fi
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
        src = wholeTree;
        nativeBuildInputs = kbuildInputs;
        env = reproEnv;
        dontFixup = true;
        postUnpack = pinBuildTreeName;
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

    # The slice map units.nix resolves srctree-relative paths through:
    # exact directory keys plus the files-only slices keyed by their
    # directory ("" for the tree root).
    sliceMap = lib.genAttrs' shardSpecs (shard: lib.nameValuePair shard.dest shard.slice);

    imported = import unitsNix {
      inherit pkgs;
      srcSlices = sliceMap;
      generated = "${generatedSnapshot}";
      extraNativeBuildInputs = kbuildInputs;
      extraEnv = reproEnv;
    };

    # The fleet cache lane's aggregation root (#3413): unit outputs are
    # build-time deps of vmlinux, so pushing any final artifact would never
    # publish the per-TU objects; this farm carries every unit output plus
    # the eval-time IFD artifacts (plan, rendered units.nix, snapshot,
    # skeleton) in one runtime closure. Build it on a CI dispatcher and the
    # post-build hook enqueues ONE obligation whose recursive closure the
    # drainer publishes -- NARs and the CA realisations another host needs to
    # substitute instead of rebuild. The mass unit build itself must run with
    # the per-derivation hook disabled (`--option post-build-hook ''` as a
    # trusted user): at 3.6k units the per-drv enqueue serialized the whole
    # build under queue backpressure (#3413 PR A).
    cachePushRoot = pkgs.linkFarm "kbuild-unit-cache-root" (
      [
        {
          name = "units";
          path = imported.allUnits;
        }
        {
          name = "units-nix";
          path = unitsNix;
        }
        {
          name = "generated";
          path = generatedSnapshot;
        }
        {
          name = "plan";
          path = plan;
        }
      ]
      ++ lib.optionals (planStrategy == "skeleton") [
        {
          name = "skeleton";
          path = skeletonTree;
        }
        {
          name = "skeleton-shards";
          path = skeletonShards;
        }
      ]
    );

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
    # Explicit exports rather than `imported // ...`: an attrset update
    # forces the import's attribute names, which walks the whole IFD chain
    # (plan build included) just to look at plan-side attrs like
    # `skeletonTree` or `srcTree`. Spelled out, only the unit-graph attrs
    # touch the import (#3706).
    {
      inherit (imported) units kernelRelease vmlinux modpost moduleSymvers modules allUnits prunedTargets;
      inherit plan unitsNix skeletonTree skeletonShards referenceKernel cachePushRoot vmlinuxEquivalence modulesEquivalence;
      srcTree =
        if srcSlices != null
        then wholeTree
        else tree;
    };
in {
  inherit buildKernel kbuildInputs;
}
