{
  lib,
  buildGoModule,
  ix,
  llvmPackages,
  stdenv,
  versionCheckHook,
}: let
  # The eBPF Makefile speaks GOARCH names and picks the clang `-target` triple
  # from them, so translate once here rather than letting `uname -m` decide:
  # a cross build has no business reading the builder's architecture.
  targetArch =
    {
      x86_64-linux = "amd64";
      aarch64-linux = "arm64";
    }
    .${
      stdenv.hostPlatform.system
    }
    or (throw "ebpf-profiler: unsupported system ${stdenv.hostPlatform.system}");
in
  buildGoModule (finalAttrs: {
    pname = "ebpf-profiler";
    # Upstream versions by ISO year+week (`v0.0.<yyyyww>`), not semver; there is
    # no stable release line yet.
    version = "0.0.202627";

    # Pinned in flake.nix as the `ebpf-profiler-src` input; the repo lint
    # (no-fetchfromgithub-fixed-hash) requires GitHub sources to be owned by a
    # flake input rather than an inline hash, so `nix flake update` moves them.
    src = ix.ebpfProfilerSrc;

    vendorHash = "sha256-pZXI3N/kBCroX2fhxEwnLiQxWE/8H1I0ycZoAV5qtmk=";

    # Only the root main package; the tree also carries a `tools/coredump`
    # command and integration-test mains that pull in cgo and testdata.
    subPackages = ["."];

    # Upstream's Makefile exports CGO_ENABLED=0 for the agent: the binary is
    # meant to be a static drop-in on any glibc or musl host, and the cgo-only
    # code paths live in the coredump tooling and tests, not in the agent.
    env.CGO_ENABLED = 0;

    # osusergo/netgo keep the pure-Go user and resolver implementations, which
    # is what makes the CGO_ENABLED=0 binary able to resolve names at all.
    tags = [
      "osusergo"
      "netgo"
    ];

    ldflags = [
      "-s"
      "-w"
    ];

    # print_instruction_count.sh runs from the eBPF Makefile's link rule, so its
    # `/usr/bin/env bash` shebang has to resolve during buildPhase. patchShebangs
    # normally only runs in fixupPhase, by which point the build has failed.
    postPatch = ''
      # shell
      patchShebangs support/ebpf/print_instruction_count.sh support/generate.sh

      # Upstream takes its version from `debug.ReadBuildInfo()`, which only
      # carries one for `go install module@version`; a build from an unpacked
      # tarball reports the literal "(devel)" in every log line and from
      # `-version`. There is no ldflags variable to stamp instead, so name the
      # version we actually built. --replace-fail keeps this honest if upstream
      # restructures the block.
      substituteInPlace main.go \
        --replace-fail 'version = buildInfo.Main.Version' 'version = "v${finalAttrs.version}"'
    '';

    nativeBuildInputs = [
      # Unwrapped clang on purpose. The nixpkgs cc-wrapper injects
      # `-nostdlibinc`, which clang reports as unused once the eBPF Makefile has
      # already passed `-nostdinc`, and that Makefile compiles with `-Werror`:
      # the wrapper's own flag fails the build. Nothing here wants the wrapper
      # anyway, because `-nostdinc -nostdlib -ffreestanding` means the only
      # headers in play are the eBPF sources' own plus clang's builtins.
      llvmPackages.clang-unwrapped
      # llvm-link, llc, llvm-strip and llvm-objdump, all four used by the eBPF
      # link rule.
      llvmPackages.llvm
    ];

    # support/support_${targetArch}.go embeds ebpf/tracer.ebpf.${targetArch}, so
    # the BPF object has to exist before `go build` runs or the embed fails.
    # The Makefile's tool variables are pinned to the `-17` suffixed binaries
    # Debian ships; nixpkgs installs them unsuffixed.
    #
    # Upstream also commits the linked object to git and its CI asserts a fresh
    # clang-17 build reproduces it byte for byte. We rebuild from the C instead
    # of embedding the committed blob, and cannot make that assertion: nixpkgs
    # ships clang 21, whose output differs. So the two hashes are logged as
    # provenance rather than checked -- this is a record, not a guard. That the
    # kernel accepts our object is verified by running it, not by comparing it.
    preBuild = ''
      # shell
      echo "eBPF object, upstream committed: $(sha256sum support/ebpf/tracer.ebpf.${targetArch})"
      make -C support/ebpf \
        TARGET_ARCH=${targetArch} \
        BPF_CLANG=clang \
        BPF_LINK=llvm-link \
        LLC=llc \
        STRIP=llvm-strip
      echo "eBPF object, rebuilt here:       $(sha256sum support/ebpf/tracer.ebpf.${targetArch})"
    '';

    # Guards the `main.go` version substitution above: without it a silently
    # failed patch ships a binary that reports "(devel)" and nobody notices.
    nativeInstallCheckInputs = [versionCheckHook];
    doInstallCheck = true;
    versionCheckProgramArg = "-version";

    # buildGoModule hands `postPatch` and `preBuild` to the vendor derivation
    # too, so without this the eBPF objects get compiled a second time while
    # fetching Go modules -- work whose output that derivation then throws away.
    overrideModAttrs = _: {preBuild = "";};

    meta = {
      description = "Whole-system eBPF CPU profiler exporting OTLP profiles";
      longDescription = ''
        The OpenTelemetry eBPF profiler samples on-CPU stacks for every process
        on a host from a single agent, unwinding native frames from .eh_frame
        and interpreter frames for Python, Ruby, PHP, Perl, Node, Java and Go,
        then ships them as OTLP profiles.

        This is the repository's standalone `ebpf-profiler` binary. Upstream's
        go.mod says outright that it builds no official artifact: the supported
        distribution is the `otelcol-ebpf-profiler` collector build, where the
        same code registers as a receiver named `profiling`. The standalone
        binary is the smaller thing to run when what you want is the agent's
        own OTLP output rather than a collector pipeline.

        Needs Linux 5.10 or newer. Upstream says CAP_SYS_ADMIN, or CAP_BPF plus
        CAP_PERFMON; measured on a NixOS host (kernel 7.1.3, kptr_restrict=1,
        unprivileged_bpf_disabled=2) the working non-root set is CAP_BPF,
        CAP_PERFMON, CAP_SYSLOG, CAP_SYS_RESOURCE, CAP_SYS_PTRACE and
        CAP_DAC_READ_SEARCH -- CAP_SYSLOG for /proc/kallsyms addresses,
        CAP_SYS_RESOURCE to raise RLIMIT_MEMLOCK, and DAC_READ_SEARCH to reach
        /sys/kernel/tracing for the sched tracepoints. CAP_SYS_ADMIN on its own
        is not enough. A unit must also leave /proc unfiltered: ProcSubset=pid
        hides /proc/kallsyms and the agent refuses to start. /sys/kernel/debug
        is not needed.
      '';
      homepage = "https://github.com/open-telemetry/opentelemetry-ebpf-profiler";
      changelog = "https://github.com/open-telemetry/opentelemetry-ebpf-profiler/releases/tag/v${finalAttrs.version}";
      # Apache-2.0 for the Go userspace; the eBPF C under support/ebpf that ends
      # up embedded in the binary is GPL-2.0-only, so the shipped artifact
      # carries both.
      license = [
        lib.licenses.asl20
        lib.licenses.gpl2Only
      ];
      mainProgram = "ebpf-profiler";
      platforms = [
        "x86_64-linux"
        "aarch64-linux"
      ];
    };
  })
