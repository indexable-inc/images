# Compiler shim for kbuild-unit plan builds (#3413), installed as `gcc` ahead
# of the real cc-wrapper on the plan derivation's PATH only. kbuild's
# $(CC) = gcc keeps resolving -- and `savedcmd_*` keeps recording -- plain
# `gcc`, so unit replays resolve the real compiler and no saved command is
# ever rewritten.
#
# Env contract (set by lib/kernel/kbuild-unit.nix; env never appears in
# savedcmd):
#   KBUILD_UNIT_CC_REAL   the real cc-wrapper gcc
#   KBUILD_UNIT_CC_MODE   "skeleton" | "ccache"
#   KBUILD_UNIT_CCACHE    ccache binary (ccache mode only)

real="$KBUILD_UNIT_CC_REAL"

if [[ "${KBUILD_UNIT_CC_MODE:-}" == ccache ]]; then
  # Correctness never depends on the cache: an unmounted or unwritable cache
  # dir (the host did not opt in via services.ci-runner.kbuildCcache) degrades
  # to an uncached build -- loudly, once per build.
  if [[ -d "${CCACHE_DIR:-}" && -w "${CCACHE_DIR:-}" ]]; then
    exec "$KBUILD_UNIT_CCACHE" "$real" "$@"
  fi
  flag="${NIX_BUILD_TOP:-/tmp}/.kbuild-unit-ccache-warned"
  if [[ ! -e $flag ]]; then
    : >"$flag"
    echo "WARNING: kbuild-unit ccache dir '${CCACHE_DIR:-}' is not mounted in this sandbox; building uncached (enable services.ci-runner.kbuildCcache on the builder)" >&2
  fi
  exec "$real" "$@"
fi

# Skeleton mode: stub the object of every reduced TU. A compile is stubbed
# only when (a) it produces an object (-c), (b) it is a kernel TU
# (-D__KERNEL__: host-tool compiles pass through), and (c) its source operand
# starts with the reducer's marker line, so the skeleton's keep decision is
# the only stub policy. cc-option probes over /dev/null, allowlisted sources,
# and non-reduced trees have no marker and pass through untouched.
compile=0
kernel=0
src=
for arg in "$@"; do
  case $arg in
    -c) compile=1 ;;
    -D__KERNEL__) kernel=1 ;;
    *.c | *.S)
      if [[ -f $arg ]]; then
        src=$arg
      fi
      ;;
  esac
done

marker=
if [[ -n $src ]]; then
  IFS= read -r marker <"$src" || true
fi
if ((!compile || !kernel)) || [[ $marker != '/* nix-kbuild-unit skeleton:'* ]]; then
  exec "$real" "$@"
fi

# 1) Dep recording: replay the original argv as -E with the object output
# discarded. The -Wp,-MMD,<depfile> already present in every kbuild compile
# argv writes the depfile fixdep consumes; the skeleton preserves every
# directive and all headers whole, so the recorded dep set is identical to a
# real build's.
dep_args=()
out_next=0
for arg in "$@"; do
  if ((out_next)); then
    out_next=0
    dep_args+=(/dev/null)
    continue
  fi
  case $arg in
    -c) dep_args+=(-E) ;;
    -o)
      out_next=1
      dep_args+=(-o)
      ;;
    *) dep_args+=("$arg") ;;
  esac
done
"$real" "${dep_args[@]}"

# 2) Stub object: the same argv over a canned stdin stub instead of the
# source, so the object carries the right machine/ABI flags for thin
# `ar cDPrST` and `ld -r` while its contents are independent of every
# function body. The stub is not empty: modpost hard-errors on a module
# object with no .modinfo license entry, so every stub carries one (the
# value is plan-only garbage; the modpost units regenerate .mod.c and
# Module.symvers from real objects at unit time). The -include args would
# drag real code back in; a surviving -Wp,-MMD would clobber the depfile
# step 1 just wrote.
stub_args=()
skip=0
for arg in "$@"; do
  if ((skip)); then
    skip=0
    continue
  fi
  case $arg in
    -include) skip=1 ;;
    -Wp,-MMD,* | -Wp,-MD,*) ;;
    "$src") ;;
    *) stub_args+=("$arg") ;;
  esac
done
if [[ $src == *.S ]]; then
  printf '%s\n' \
    '.section .modinfo,"a"' \
    '.ascii "license=GPL"' \
    '.byte 0' \
    | "$real" "${stub_args[@]}" -x assembler-with-cpp -
else
  printf '%s\n' \
    'static const char __attribute__((section(".modinfo"), used))' \
    'kbuild_unit_stub_license[] = "license=GPL";' \
    | "$real" "${stub_args[@]}" -x c -
fi
