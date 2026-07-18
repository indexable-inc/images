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
# Env contract (set by lib/kernel/kbuild-unit.nix):
#   KBUILD_UNIT_LD_REAL   the real binutils ld-wrapper

real="$KBUILD_UNIT_LD_REAL"

out=
grab=0
for arg in "$@"; do
  if ((grab)); then
    out=$arg
    grab=0
  elif [[ $arg == -o ]]; then
    grab=1
  fi
done

case ${out##*/} in
  vmlinux | .tmp_vmlinux*) ;;
  *) exec "$real" "$@" ;;
esac

exec "$real" \
  --defsym jiffies_64=0 \
  --defsym startup_64=0 \
  --defsym startup_32=0 \
  --defsym gdt_page=0 \
  --defsym fixed_percpu_data=0 \
  --defsym irq_stack_backing_store=0 \
  --defsym retbleed_return_thunk=0 \
  --defsym srso_safe_ret=0 \
  --defsym srso_alias_untrain_ret=0 \
  --defsym srso_alias_safe_ret=0x104104 \
  --defsym __x86_indirect_its_thunk_rax=0x20 \
  --defsym __x86_indirect_its_thunk_rcx=0x60 \
  --defsym __x86_indirect_its_thunk_array=0x20 \
  --defsym its_return_thunk=0x20 \
  --unresolved-symbols=ignore-all \
  "$@"
