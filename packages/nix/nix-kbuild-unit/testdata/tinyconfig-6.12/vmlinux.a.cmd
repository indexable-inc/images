savedcmd_vmlinux.a := rm -f vmlinux.a; ar cDPrST vmlinux.a ./built-in.a; ar mPiT $$(ar t vmlinux.a | sed -n 1p) vmlinux.a $$(ar t vmlinux.a | grep -F -f ./scripts/head-object-list.txt)
