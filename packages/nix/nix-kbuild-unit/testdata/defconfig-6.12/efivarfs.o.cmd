savedcmd_fs/efivarfs/efivarfs.o := ld -m elf_x86_64 -z noexecstack --no-warn-rwx-segments   -r -o fs/efivarfs/efivarfs.o @fs/efivarfs/efivarfs.mod  ; ./tools/objtool/objtool --hacks=jump_label --hacks=noinstr --hacks=skylake --ibt --orc --retpoline --rethunk --static-call --uaccess --prefix=16  --link  --module fs/efivarfs/efivarfs.o

fs/efivarfs/efivarfs.o: $(wildcard ./tools/objtool/objtool)
