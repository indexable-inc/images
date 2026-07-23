savedcmd_arch/x86/kernel/vmlinux.lds := gcc -E -Wp,-MMD,arch/x86/kernel/.vmlinux.lds.d -nostdinc -I./arch/x86/include -I./arch/x86/include/generated  -I./include -I./arch/x86/include/uapi -I./arch/x86/include/generated/uapi -I./include/uapi -I./include/generated/uapi -include ./include/linux/compiler-version.h -include ./include/linux/kconfig.h -D__KERNEL__ -fmacro-prefix-map=./=  -Ui386 -P -Ux86 -D__ASSEMBLY__ -DLINKER_SCRIPT -o arch/x86/kernel/vmlinux.lds arch/x86/kernel/vmlinux.lds.S

source_arch/x86/kernel/vmlinux.lds := arch/x86/kernel/vmlinux.lds.S

deps_arch/x86/kernel/vmlinux.lds := \
    $(wildcard include/config/OUTPUT_FORMAT) \
    $(wildcard include/config/X86_32) \
    $(wildcard include/config/X86_64) \
    $(wildcard include/config/SMP) \
    $(wildcard include/config/MITIGATION_RETPOLINE) \
    $(wildcard include/config/MITIGATION_SRSO) \
    $(wildcard include/config/PHYSICAL_START) \
    $(wildcard include/config/X86_INTEL_MID) \
    $(wildcard include/config/X86_KERNEL_IBT) \
    $(wildcard include/config/FINEIBT) \
    $(wildcard include/config/AMD_MEM_ENCRYPT) \
    $(wildcard include/config/KASAN) \
    $(wildcard include/config/COMPILE_TEST) \
    $(wildcard include/config/MITIGATION_UNRET_ENTRY) \
    $(wildcard include/config/MITIGATION_ITS) \
    $(wildcard include/config/DEBUG_FORCE_FUNCTION_ALIGN_64B) \
  include/linux/compiler-version.h \
    $(wildcard include/config/CC_VERSION_TEXT) \
  include/linux/kconfig.h \
    $(wildcard include/config/CPU_BIG_ENDIAN) \
    $(wildcard include/config/BOOGER) \
    $(wildcard include/config/FOO) \
  include/asm-generic/vmlinux.lds.h \
    $(wildcard include/config/FUNCTION_ALIGNMENT) \
    $(wildcard include/config/LD_DEAD_CODE_DATA_ELIMINATION) \
    $(wildcard include/config/LTO_CLANG) \
    $(wildcard include/config/HAVE_DYNAMIC_FTRACE_NO_PATCHABLE) \
    $(wildcard include/config/ARCH_SUPPORTS_CFI_CLANG) \
    $(wildcard include/config/FTRACE_MCOUNT_RECORD) \
    $(wildcard include/config/FUNCTION_TRACER) \
    $(wildcard include/config/TRACE_BRANCH_PROFILING) \
    $(wildcard include/config/PROFILE_ALL_BRANCHES) \
    $(wildcard include/config/KPROBES) \
    $(wildcard include/config/FUNCTION_ERROR_INJECTION) \
    $(wildcard include/config/EVENT_TRACING) \
    $(wildcard include/config/TRACING) \
    $(wildcard include/config/FTRACE_SYSCALLS) \
    $(wildcard include/config/BPF_EVENTS) \
    $(wildcard include/config/SERIAL_EARLYCON) \
    $(wildcard include/config/SECURITY) \
    $(wildcard include/config/TIMER_OF) \
    $(wildcard include/config/IRQCHIP) \
    $(wildcard include/config/COMMON_CLK) \
    $(wildcard include/config/OF_RESERVED_MEM) \
    $(wildcard include/config/CPU_IDLE) \
    $(wildcard include/config/ACPI) \
    $(wildcard include/config/THERMAL) \
    $(wildcard include/config/HAVE_STATIC_CALL_INLINE) \
    $(wildcard include/config/ARCH_USES_CFI_TRAPS) \
    $(wildcard include/config/DEBUG_INFO_BTF) \
    $(wildcard include/config/CONSTRUCTORS) \
    $(wildcard include/config/GENERIC_BUG) \
    $(wildcard include/config/UNWINDER_ORC) \
    $(wildcard include/config/FW_LOADER) \
    $(wildcard include/config/PM_TRACE) \
    $(wildcard include/config/PRINTK_INDEX) \
    $(wildcard include/config/BLK_DEV_INITRD) \
    $(wildcard include/config/UNWIND_TABLES) \
    $(wildcard include/config/GCOV_KERNEL) \
    $(wildcard include/config/KASAN_GENERIC) \
    $(wildcard include/config/KCSAN) \
  include/asm-generic/codetag.lds.h \
  arch/x86/include/asm/asm-offsets.h \
  include/generated/asm-offsets.h \
  arch/x86/include/asm/thread_info.h \
    $(wildcard include/config/VM86) \
    $(wildcard include/config/X86_FRED) \
    $(wildcard include/config/X86_IOPL_IOPERM) \
    $(wildcard include/config/FRAME_POINTER) \
    $(wildcard include/config/COMPAT) \
    $(wildcard include/config/IA32_EMULATION) \
  include/linux/compiler.h \
    $(wildcard include/config/OBJTOOL) \
    $(wildcard include/config/64BIT) \
  include/linux/compiler_types.h \
    $(wildcard include/config/PAHOLE_HAS_BTF_TAG) \
    $(wildcard include/config/CC_HAS_SANE_FUNCTION_ALIGNMENT) \
    $(wildcard include/config/ARM64) \
    $(wildcard include/config/HAVE_ARCH_COMPILER_H) \
    $(wildcard include/config/CC_HAS_COUNTED_BY) \
    $(wildcard include/config/UBSAN_SIGNED_WRAP) \
    $(wildcard include/config/CC_HAS_ASM_INLINE) \
  arch/x86/include/generated/asm/rwonce.h \
  include/asm-generic/rwonce.h \
  arch/x86/include/asm/page.h \
  include/linux/types.h \
    $(wildcard include/config/HAVE_UID16) \
    $(wildcard include/config/UID16) \
    $(wildcard include/config/ARCH_DMA_ADDR_T_64BIT) \
    $(wildcard include/config/PHYS_ADDR_T_64BIT) \
    $(wildcard include/config/ARCH_32BIT_USTAT_F_TINODE) \
  include/uapi/linux/types.h \
  arch/x86/include/generated/uapi/asm/types.h \
  include/uapi/asm-generic/types.h \
  include/asm-generic/int-ll64.h \
  include/uapi/asm-generic/int-ll64.h \
  arch/x86/include/uapi/asm/bitsperlong.h \
  include/asm-generic/bitsperlong.h \
  include/uapi/asm-generic/bitsperlong.h \
  arch/x86/include/asm/page_types.h \
    $(wildcard include/config/PAGE_SHIFT) \
    $(wildcard include/config/PHYSICAL_ALIGN) \
    $(wildcard include/config/DYNAMIC_PHYSICAL_MASK) \
  include/linux/const.h \
  include/vdso/const.h \
  include/uapi/linux/const.h \
  include/linux/mem_encrypt.h \
    $(wildcard include/config/ARCH_HAS_MEM_ENCRYPT) \
  arch/x86/include/asm/page_32_types.h \
    $(wildcard include/config/HIGHMEM4G) \
    $(wildcard include/config/HIGHMEM64G) \
    $(wildcard include/config/PAGE_OFFSET) \
    $(wildcard include/config/X86_PAE) \
  arch/x86/include/asm/page_32.h \
    $(wildcard include/config/DEBUG_VIRTUAL) \
  include/asm-generic/memory_model.h \
    $(wildcard include/config/FLATMEM) \
    $(wildcard include/config/SPARSEMEM_VMEMMAP) \
    $(wildcard include/config/SPARSEMEM) \
  include/linux/pfn.h \
  include/asm-generic/getorder.h \
  arch/x86/include/asm/percpu.h \
    $(wildcard include/config/X86_64_SMP) \
    $(wildcard include/config/CC_HAS_NAMED_AS) \
    $(wildcard include/config/USE_X86_SEG_SUPPORT) \
    $(wildcard include/config/UML) \
  arch/x86/include/asm/orc_lookup.h \
  arch/x86/include/asm/cache.h \
    $(wildcard include/config/X86_L1_CACHE_SHIFT) \
    $(wildcard include/config/X86_INTERNODE_CACHE_SHIFT) \
    $(wildcard include/config/X86_VSMP) \
  include/linux/linkage.h \
    $(wildcard include/config/ARCH_USE_SYM_ANNOTATIONS) \
  include/linux/stringify.h \
  include/linux/export.h \
    $(wildcard include/config/MODVERSIONS) \
  arch/x86/include/asm/linkage.h \
    $(wildcard include/config/CALL_PADDING) \
    $(wildcard include/config/MITIGATION_RETHUNK) \
    $(wildcard include/config/MITIGATION_SLS) \
    $(wildcard include/config/FUNCTION_PADDING_BYTES) \
  arch/x86/include/asm/ibt.h \
  arch/x86/include/asm/boot.h \
    $(wildcard include/config/KERNEL_BZIP2) \
    $(wildcard include/config/KERNEL_ZSTD) \
  arch/x86/include/asm/pgtable_types.h \
    $(wildcard include/config/X86_INTEL_MEMORY_PROTECTION_KEYS) \
    $(wildcard include/config/MEM_SOFT_DIRTY) \
    $(wildcard include/config/HAVE_ARCH_USERFAULTFD_WP) \
    $(wildcard include/config/PGTABLE_LEVELS) \
    $(wildcard include/config/PROC_FS) \
  arch/x86/include/asm/pgtable_32_types.h \
  arch/x86/include/asm/pgtable-2level_types.h \
  arch/x86/include/uapi/asm/boot.h \
  arch/x86/include/asm/vvar.h \

arch/x86/kernel/vmlinux.lds: $(deps_arch/x86/kernel/vmlinux.lds)

$(deps_arch/x86/kernel/vmlinux.lds):
