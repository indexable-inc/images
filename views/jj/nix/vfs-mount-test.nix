# Mounts a jj revision inside a NixOS VM and checks the result through real
# kernel filesystem clients, once over FUSE and once over the kernel's own NFSv3
# client. The NFS half is the important one: it exercises our server end to end
# against a client we did not write, which is the only way to know the wire
# format is right rather than merely self-consistent.
#
# The mounts come from the NixOS module rather than being stood up by hand, so
# what is under test is what a host would actually run: the unit's ordering, its
# restart policy and its teardown path, none of which a hand-rolled `systemd-run`
# would exercise.
#
# This covers Linux only. The macOS mount cannot be covered here or anywhere in
# CI; see docs/vfs.md for the manual recipe and why.
#
# Exposed as `packages.<system>.vfs-mount-test` rather than as a flake check,
# because GitHub hosted runners have the nixos-test feature but not kvm, and a
# VM test without kvm fails outright rather than falling back to emulation.
# `nix flake check` builds checks but not packages, so this runs only where
# someone asks for it: `nix build .#vfs-mount-test` on a host with KVM.
{
  testers,
  fuse3,
  nfs-utils,
  jujutsu,
  jjVfsModule,
}:
testers.runNixOSTest {
  name = "jj-vfs-mount";

  nodes.machine = {...}: {
    imports = [jjVfsModule];

    services.jj-vfs = {
      package = jujutsu;
      mounts = {
        fuse = {
          repository = "/root/repo";
          mountPoint = "/mnt/fuse";
          transport = "fuse";
          # The repository is created by the test script, so it does not exist
          # at boot and the unit would fail before the script could run. This is
          # the option that exists for exactly that case.
          startAtBoot = false;
        };
        nfs = {
          repository = "/root/repo";
          mountPoint = "/mnt/nfs";
          transport = "nfs";
          nfsPort = 20049;
          startAtBoot = false;
        };
      };
    };

    environment.systemPackages = [
      jujutsu
      # fusermount3 is the unprivileged mount path. The test runs as root and so
      # takes fuser's direct mount syscall instead, but having it present means
      # the fallback a non-root user would hit is exercised as present rather
      # than absent.
      fuse3
      nfs-utils
    ];
    virtualisation.memorySize = 2048;
  };

  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target")

    env = "JJ_USER=Test JJ_EMAIL=test@example.com HOME=/root"

    def jj(args, cwd="/root/repo"):
        return machine.succeed(f"cd {cwd} && {env} jj {args}")

    machine.succeed("mkdir -p /root/repo")
    jj("git init .")
    machine.succeed("printf 'hello from the mount\\n' > /root/repo/hello.txt")
    machine.succeed("printf '#!/bin/sh\\necho ran\\n' > /root/repo/run.sh")
    machine.succeed("chmod +x /root/repo/run.sh")
    machine.succeed("mkdir -p /root/repo/sub")
    machine.succeed("printf 'nested content\\n' > /root/repo/sub/inner.txt")
    machine.succeed("ln -s hello.txt /root/repo/link")
    # Two names differing only in case. ext4 here is case-sensitive so both can
    # exist in the working copy; the point is that the mount keeps them apart.
    # Nix's use-case-hack bakes a ~nix~case~hack~ suffix into the NAR when it sees
    # a case-insensitive collision, so a case-folding mount would make store paths
    # computed on a Mac diverge from Linux for identical content.
    machine.succeed("printf 'upper\\n' > /root/repo/Case")
    machine.succeed("printf 'lower\\n' > /root/repo/case")
    # A file big enough to need more than one NFS read, so offset handling is
    # exercised rather than assumed.
    machine.succeed("head -c 300000 /dev/zero | tr '\\0' 'x' > /root/repo/big.txt")
    jj("describe -m 'vfs test'")

    # The expected hashes come out of the repository rather than the working
    # copy, so the comparison is against what jj thinks the revision holds.
    expected_hello = jj("file show -r @ hello.txt | sha256sum").split()[0]
    expected_inner = jj("file show -r @ sub/inner.txt | sha256sum").split()[0]
    expected_big = jj("file show -r @ big.txt | sha256sum").split()[0]

    def check_mount(transport, mnt, round):
        with subtest(f"{transport}: listing"):
            listing = sorted(machine.succeed(f"ls -A {mnt}").split())
            assert listing == [
                "Case", "big.txt", "case", "hello.txt", "link", "run.sh", "sub",
            ], listing
            # find has to walk the whole tree, which fails if a directory
            # reports a type or a link count a traversal chokes on. %P of the
            # starting point is empty and the output ends in a newline, so both
            # empty strings are dropped rather than asserted about.
            found = sorted(
                path
                for path in machine.succeed(f"find {mnt} -printf '%P\\n'").split("\n")
                if path
            )
            assert found == [
                "Case", "big.txt", "case", "hello.txt", "link", "run.sh", "sub",
                "sub/inner.txt",
            ], found

        with subtest(f"{transport}: contents match the repository"):
            assert machine.succeed(f"cat {mnt}/hello.txt") == "hello from the mount\n"
            got = machine.succeed(f"sha256sum < {mnt}/hello.txt").split()[0]
            assert got == expected_hello, f"hello.txt: {got} != {expected_hello}"
            got = machine.succeed(f"sha256sum < {mnt}/sub/inner.txt").split()[0]
            assert got == expected_inner, f"sub/inner.txt: {got} != {expected_inner}"
            # 300 kB is several reads, so a wrong offset or a wrong EOF flag
            # shows up as a hash mismatch here and nowhere else.
            got = machine.succeed(f"sha256sum < {mnt}/big.txt").split()[0]
            assert got == expected_big, f"big.txt: {got} != {expected_big}"
            size = machine.succeed(f"stat -c %s {mnt}/big.txt").strip()
            assert size == "300000", size

        with subtest(f"{transport}: symlink"):
            assert machine.succeed(f"readlink {mnt}/link").strip() == "hello.txt"
            # Following it proves the kernel accepted it as a symlink rather
            # than as a regular file holding a path.
            assert machine.succeed(f"cat {mnt}/link") == "hello from the mount\n"

        with subtest(f"{transport}: executable bit"):
            # The mode is what a tool reads, and it is right on both transports.
            assert machine.succeed(f"stat -c %a {mnt}/run.sh").strip() == "555"
            assert machine.succeed(f"stat -c %a {mnt}/hello.txt").strip() == "444"
            machine.succeed(f"test -x {mnt}/run.sh")
            assert machine.succeed(f"{mnt}/run.sh") == "ran\n"
            # access(2) has to agree with the mode, or anything asking "is this
            # runnable" gets the wrong answer. This holds on both transports
            # here: over FUSE because the mount carries default_permissions, and
            # over NFS because the Linux client checks locally against the
            # attributes it cached. macOS's NFS client instead trusts the ACCESS
            # reply, which nfs3_server sends without consulting the mode, so this
            # assertion does not hold there. See docs/vfs.md and ENG-11614; no
            # test here can catch that, since no CI runner is a Mac.
            machine.succeed(f"test ! -x {mnt}/hello.txt")

        with subtest(f"{transport}: names differing only in case stay distinct"):
            # Distinct content is the assertion that matters: a case-folding mount
            # would serve one of these for both names, and `ls` alone would not
            # necessarily show it.
            assert machine.succeed(f"cat {mnt}/Case") == "upper\n"
            assert machine.succeed(f"cat {mnt}/case") == "lower\n"

        with subtest(f"{transport}: dot-dot walks back up"):
            # `pwd -P` calls getcwd(3), which resolves the path by walking ".."
            # entries through the filesystem rather than using the shell's
            # logical cwd, so this fails if ".." points anywhere wrong.
            got = machine.succeed(f"cd {mnt}/sub && pwd -P").strip()
            assert got == f"{mnt}/sub", got
            got = machine.succeed(f"cd {mnt}/sub && cd .. && pwd -P").strip()
            assert got == mnt, got
            up = sorted(machine.succeed(f"ls -A {mnt}/sub/..").split())
            assert up == [
                "Case", "big.txt", "case", "hello.txt", "link", "run.sh", "sub",
            ], up

        with subtest(f"{transport}: flock works on the mount"):
            # jj takes its own locks with flock (lib/src/lock/unix.rs), so a
            # mount where flock returns ENOTSUP cannot host anything jj touches.
            # On macOS the mount option "nolocks" causes exactly that, which is
            # why we pass "locallocks" instead; this asserts the Linux side.
            # Opening through the shell keeps it read-only, since flock(1) would
            # otherwise try to create the file on a read-only mount.
            machine.succeed(f"sh -c 'exec 9< {mnt}/hello.txt; flock -n 9'")

        with subtest(f"{transport}: the mount is read-only"):
            machine.fail(f"touch {mnt}/newfile")
            machine.fail(f"sh -c 'echo x >> {mnt}/hello.txt'")

    def run_transport(transport, round):
        mnt = f"/mnt/{transport}"
        unit = f"jj-vfs-{transport}"
        # `systemctl start`, not systemd-run: the unit under test is the one the
        # module generates, so its ordering, its restart policy and its
        # ExecStopPost teardown are all exercised rather than bypassed.
        try:
            machine.succeed(f"systemctl start {unit}")
            machine.wait_until_succeeds(f"mountpoint -q {mnt}", timeout=60)
        except Exception:
            # Without this the only evidence of a failed mount is a bare
            # timeout: jj's own error message goes to the unit's journal, and
            # nothing else in the test ever prints it.
            print(machine.execute(f"journalctl -u {unit} --no-pager")[1])
            print(machine.execute(f"systemctl status {unit}")[1])
            raise
        print(machine.succeed(f"mount | grep {mnt}"))
        check_mount(transport, mnt, round)
        with subtest(f"{transport} round {round}: SIGTERM unmounts cleanly"):
            machine.succeed(f"systemctl stop {unit}")
            # Both waited on rather than checked once: umount returning and the
            # entry leaving the mount table are not the same instant. A
            # mountpoint left behind is the failure this asserts against, and it
            # has to be gone quickly, not eventually: a wedged NFS mount does
            # disappear once the client gives up on the dead server, so a
            # generous timeout here would pass on exactly the broken case.
            machine.wait_until_fails(f"mountpoint -q {mnt}", timeout=15)
            machine.wait_until_fails(f"mount | grep -q {mnt}", timeout=15)
        print(machine.succeed(f"journalctl -u {unit} --no-pager | tail -20"))

    # Mount and unmount several times per run rather than once. Tearing down an
    # NFS mount races the server's own exit, and a race that wins once wins in a
    # fresh VM too: the bug this catches passed three whole runs before it
    # failed on the fourth. Looping in-process costs seconds where another VM
    # boot costs minutes.
    for round in range(1, 4):
        run_transport("fuse", round)
        run_transport("nfs", round)
  '';
}
