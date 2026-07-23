savedcmd_vmlinux := scripts/link-vmlinux.sh "ld" "-m elf_i386 -z noexecstack --no-warn-rwx-segments" "--build-id=sha1 --orphan-handling=warn";  make -f ./arch/x86/Makefile.postlink vmlinux

source_vmlinux := scripts/link-vmlinux.sh

deps_vmlinux := \
    $(wildcard include/config/LTO_CLANG) \
    $(wildcard include/config/X86_KERNEL_IBT) \
    $(wildcard include/config/MODULES) \
    $(wildcard include/config/VMLINUX_MAP) \
    $(wildcard include/config/CPU_BIG_ENDIAN) \
    $(wildcard include/config/KALLSYMS_ALL) \
    $(wildcard include/config/KALLSYMS_ABSOLUTE_PERCPU) \
    $(wildcard include/config/KALLSYMS) \
    $(wildcard include/config/DEBUG_INFO_BTF) \
    $(wildcard include/config/SHELL) \
    $(wildcard include/config/BUILDTIME_TABLE_SORT) \

vmlinux: $(deps_vmlinux)

$(deps_vmlinux):
