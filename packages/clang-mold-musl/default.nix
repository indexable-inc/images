# `clang-mold-musl`: the linker driver `.cargo/config.toml` invokes for the
# `x86_64-unknown-linux-musl` target. Wraps the cross-musl clang (with the right
# musl sysroot, crt files, and libc.a baked in by the nixpkgs cc-wrapper) and
# forces mold as the underlying linker.
#
# rustc invokes the "linker" as a C compiler driver (set
# `link-self-contained=-linker` so rustc does not bundle lld). The driver picks
# up musl crt + libc from the cc-wrapper; `-fuse-ld=mold` swaps the slow default
# linker for mold.
#
# Set IX_LINKER=wild to use the wild linker instead of mold (experimental).
#
# mold is kept, and the reason is not the one you would guess. Measured on
# dev-compute-6 (2026-08-04) linking `orchestrator`, the largest binary in the
# workspace at 261 MB, three reps each, timing the linker process alone:
#
#   mold 2.41.0   0.68s   1.77 GB peak RSS
#   lld  21.1.8   0.69s   1.55 GB
#   wild 0.9.0    0.70s   1.32 GB
#
# All three are within 3% on wall time, so the usual argument for mold does not
# apply here: at this size the link is I/O bound, not linker bound, and mold's
# advantage over bfd/gold does not reproduce against its modern peers. mold is
# also the *heaviest* of the three, by 34% over wild.
#
# It stays because none of that matters at the scale it is measured against.
# The same relink step peaks at 15.05 GB, of which rustc is 14.55 GB and the
# linker 1.3 to 1.8 GB. Swapping mold for the lightest alternative moves the
# step's peak by about 1.5%, which buys nothing and costs a toolchain change.
#
# This is worth stating because it was nearly the wrong conclusion. The vc1
# pressure spike on 2026-08-04 (load 185, psi_memory_full 87.2%, 11 minutes,
# while the host served production) looked like a link-phase problem, and it is
# not: `max-jobs = 32` on a 503 GB host, against build steps peaking near 15 GB
# each, is the actual mechanism. No linker choice fixes that. ENG-12260.
#
# Two traps if you re-measure. mold and wild both fork by default, and the
# child's memory never reaches the parent's rusage, so `time -v` reports about
# 12 MB for mold rather than 1.77 GB: pass `--no-fork` to either. And measure
# the linker process, not the cargo invocation around it, or you are timing
# rustc.
#
# Revisit if the debuginfo path changes enough that the linker becomes the
# dominant term in the step, or if wild (0.9.0, pre-1.0) grows a real wall-time
# win rather than the 3% loss measured here.
{
  ix,
  pkgsCross,
  mold,
  wild,
}: let
  writeBashApplication = ix.writeBashApplication ix.pkgs;
in
  writeBashApplication {
    name = "clang-mold-musl";
    runtimeInputs = [
      pkgsCross.musl64.buildPackages.clang
      mold
      wild
    ];
    # The linker invocation from rustc includes "-no-pie" which clang warns about
    # as unused; leave -u/-e/-o defaults alone and don't fail on that.
    text = ''
      linker="''${IX_LINKER:-mold}"
      exec x86_64-unknown-linux-musl-clang "-fuse-ld=$linker" "$@"
    '';
    meta.description = "clang driver for the x86_64-unknown-linux-musl Rust target, linking with mold";
  }
