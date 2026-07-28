# Linker shim for kbuild-unit skeleton plan builds (#3413), installed as `ld`
# ahead of the real ld-wrapper on the plan derivation's PATH only (savedcmd_*
# keeps recording plain `ld`, so unit replays resolve the strict real linker
# over fully real objects).
#
# The skeleton plan's stub objects define no symbols, but the x86 vmlinux
# linker script evaluates symbol expressions eagerly (`jiffies = jiffies_64;`,
# `INIT_PER_CPU(gdt_page)`, mitigation-thunk ASSERTs) and GNU ld hard-errors
# on an undefined symbol in an expression. Injecting `--defsym`s for exactly
# that symbol set (values chosen to satisfy the 6.12 ASSERT arithmetic) plus
# `--unresolved-symbols=ignore-all` (for stray real objects a keep glob pulls
# into the stub link) lets the garbage plan-time vmlinux link complete, which
# is all the plan needs: its .cmd files, not its bytes. Injection is gated to
# vmlinux-ish outputs so real plan-time links (vdso.so, realmode.elf) stay
# byte-exact for the generated snapshot.
#
# A `--keep`/skeletonKeep glob covering a TU that really defines one of these
# symbols fails the plan link loudly with a duplicate-definition error; drop
# the keep or this defsym, they cannot coexist.
#
# The stub objects also leave the output without sections the post-link
# tools insist on (sorttable hard-fails on a missing __ex_table), so the
# injected link appends one synthetic placeholder object carrying minimal
# one-entry versions; the values are garbage, and only the discarded stub
# vmlinux ever contains them.
#
# Env contract (set by lib/kernel/kbuild-unit.nix):
#   KBUILD_UNIT_LD_REAL              the real binutils ld-wrapper
#   KBUILD_UNIT_LD_PLACEHOLDER_DIR   dir with placeholder-{32,64}.o

real="$KBUILD_UNIT_LD_REAL"

out=
emulation=
script=
grab=
for arg in "$@"; do
  case $grab in
    -o) out=$arg ;;
    -m) emulation=$arg ;;
    -T) script=$arg ;;
  esac
  grab=
  case $arg in
    -o | -m | -T) grab=$arg ;;
    # link-vmlinux.sh passes the attached form (--script=<objtree>/<lds>).
    --script=*) script=${arg#--script=} ;;
    -T?*) script=${arg#-T} ;;
  esac
done

case ${out##*/} in
  vmlinux | .tmp_vmlinux*) ;;
  *) exec "$real" "$@" ;;
esac

# The SRSO branch of the script assigns the location counter from
# srso_alias_untrain_ret (`. = srso_alias_untrain_ret | 0x104104`), which an
# absolute --defsym cannot drive (ld converts the absolute value through the
# section's load-address space and errors "cannot move location counter
# backwards"). Upstream makes the OR-math work through section placement of
# the pair, so the SRSO stand-in object replicates exactly that; it is
# appended only when the generated script actually places the rethunk
# sections, because on other configs they would be orphans.
extra=("$KBUILD_UNIT_LD_PLACEHOLDER_DIR/placeholder-$([[ $emulation == elf_i386 ]] && echo 32 || echo 64).o")
if [[ -n $script && -f $script ]] && grep -q 'rethunk_safe' "$script"; then
  extra+=("$KBUILD_UNIT_LD_PLACEHOLDER_DIR/placeholder-srso-64.o")
fi

exec "$real" \
  --defsym jiffies_64=0 \
  --defsym pcpu_hot=0 \
  --defsym startup_64=0 \
  --defsym startup_32=0 \
  --defsym gdt_page=0 \
  --defsym fixed_percpu_data=0 \
  --defsym irq_stack_backing_store=0 \
  --defsym retbleed_return_thunk=0 \
  --defsym srso_safe_ret=0 \
  --defsym __x86_indirect_its_thunk_rax=0x20 \
  --defsym __x86_indirect_its_thunk_rcx=0x60 \
  --defsym __x86_indirect_its_thunk_array=0x20 \
  --defsym its_return_thunk=0x20 \
  --unresolved-symbols=ignore-all \
  "$@" \
  "${extra[@]}"
