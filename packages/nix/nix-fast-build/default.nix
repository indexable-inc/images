{pkgs}: let
  # nix-fast-build's --skip-cached drops a job only when nix-eval-jobs reports
  # its `cacheStatus` as `cached` (present in a configured *remote* substituter).
  # A `local` status (output already realized in *this* runner's store, but not
  # pushed anywhere) falls through to the build queue and gets re-realized every
  # run (workers.py, the `elif cache_status == "local"` arm).
  #
  # On index's persistent warm CI runner that is almost everything: the rust
  # workspace units default to `contentAddressed = true` (lib/rust/cargo-unit.nix)
  # and the image checks are floating-CA system closures (lib/per-system.nix), and
  # none of them are pushed to cache.ix.dev, so the patched nix-eval-jobs
  # (packages/nix/nix-eval-jobs) correctly resolves them to `local` -- at which
  # point stock nix-fast-build re-realizes all ~1450 of them. Each is a no-op
  # realize (~0.05-0.4s measured warm) but the per-drv dispatch over ~1450 units
  # is the ~85s build-step floor.
  #
  # The patch makes `local` skip the build like `cached` does (still queueing an
  # upload first if a binary-cache target is set): a locally-present output never
  # needs (re)building. nix-eval-jobs#403 / nix#12128 fixed the *status*; this
  # fixes what --skip-cached does with it.
  package = pkgs.nix-fast-build.overrideAttrs (old: {
    patches = (old.patches or []) ++ [./skip-local.patch];
  });

  # The patch only touches Python control flow, so the real risk is that the
  # surrounding source drifted out from under the diff on a nixpkgs bump (the
  # patch would fail to apply at build time) -- a build of `package` already
  # catches that. The smoke test additionally runs the binary so an import-time
  # break surfaces here rather than mid-CI-run; `--help` exits 0 without touching
  # a store or daemon (absent in the sandbox).
  smoke =
    pkgs.runCommand "nix-fast-build-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      help=$(nix-fast-build --help 2>&1) || true
      # --fail-fast is what the check gate passes (lib/per-system.nix); its
      # absence from usage is exactly the failure mode that broke CI when the
      # flag was assumed present on 1.5.0 (#2128), so assert both flags.
      case "$help" in
        *"--skip-cached"*) ;;
        *)
          echo "nix-fast-build --help did not print usage" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      case "$help" in
        *"--fail-fast"*) ;;
        *)
          echo "nix-fast-build --help lacks --fail-fast (version < 1.6.0?)" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit smoke;
          };
      };
    meta =
      (old.meta or {})
      // {
        description = "nix-fast-build patched so --skip-cached also skips locally-realized (floating-CA) outputs, not just remotely-cached ones";
        mainProgram = "nix-fast-build";
      };
  })
