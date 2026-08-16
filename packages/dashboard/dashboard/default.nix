{ix, ...}: let
  # The dashboard UI is a Svelte/Vite app under dashboard-core/site. Nix builds
  # it to one self-contained index.html (viteSingleFile) and the dashboard-core
  # build script embeds it at compile time via IX_DASHBOARD_SITE_HTML (wired in
  # lib/rust/workspace.nix), so this aggregator and the in-process tui::serve
  # carry the page with no committed artifact and no runtime asset dependency.
  unit = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "dashboard";
    meta.mainProgram = "dashboard";
  };
in
  # A plain `//` update, not `overrideAttrs`: `selectBinaryWithTests` attaches
  # its tests the same way, outside the derivation's override chain, so an
  # `overrideAttrs` here reads the *underlying* drv's empty `passthru.tests`
  # and silently drops the crate's own gate (clippy, unit tests) from
  # `ciChecks.rust-dashboard` -- which is exactly what happened until the
  # first unit test landed in this crate and never ran in CI.
  unit
  // {
    passthru =
      unit.passthru
      // {
        tests =
          unit.passthru.tests
          // {
            # Expose the nix-built site for inspection / as a build check.
            site = ix.rustWorkspace.dashboardSite;
          };
      };
  }
