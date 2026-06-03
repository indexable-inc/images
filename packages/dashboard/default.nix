{
  ix,
  ...
}:
let
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
unit.overrideAttrs {
  passthru = unit.passthru // {
    tests = unit.passthru.tests // {
      # Expose the nix-built site for inspection / as a build check.
      site = ix.rustWorkspace.dashboardSite;
    };
  };
}
