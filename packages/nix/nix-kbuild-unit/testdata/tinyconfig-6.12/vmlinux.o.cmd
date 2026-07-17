savedcmd_vmlinux.o := ld -m elf_i386 -z noexecstack --no-warn-rwx-segments -r -o vmlinux.o   --whole-archive vmlinux.a --no-whole-archive --start-group lib/lib.a arch/x86/lib/lib.a --end-group 
