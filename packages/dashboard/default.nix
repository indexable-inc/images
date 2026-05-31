{
  ix,
  lib,
  pkgs,
  ...
}:
let
  fs = lib.fileset;

  # The dashboard UI is a Svelte/Vite app under dashboard-core/site. Its
  # production build is a single self-contained index.html (viteSingleFile) that
  # is committed as dashboard-core/src/dashboard/dashboard.html and embedded via
  # include_str! so both this aggregator and the in-process tui::serve carry the
  # page with no runtime asset dependency.
  siteRoot = ../dashboard-core/site;
  siteSrc = fs.toSource {
    root = siteRoot;
    fileset = fs.intersection (fs.gitTracked ../dashboard-core) siteRoot;
  };
  site = ix.buildSvelteSite pkgs {
    pname = "dashboard-site";
    version = "0.1.0";
    src = siteSrc;
    serve.enable = false;
    devServer = {
      name = "dashboard-site-dev";
      checkoutSubdir = "packages/dashboard-core/site";
    };
  };

  # Guard against the committed artifact drifting from its source: the embedded
  # HTML must byte-match a fresh build of the site. Regenerate on failure with:
  #   nix build .#dashboard.passthru.tests.site
  #   cp result/share/dashboard-site/index.html \
  #     packages/dashboard-core/src/dashboard/dashboard.html
  committed = ../dashboard-core/src/dashboard/dashboard.html;
  dashboardInSync = pkgs.runCommand "dashboard-html-in-sync" { } ''
    if diff -u ${committed} ${site}/share/dashboard-site/index.html; then
      touch "$out"
    else
      echo "" >&2
      echo "committed dashboard.html is stale; rebuild the site and copy" >&2
      echo "its index.html over packages/dashboard-core/src/dashboard/dashboard.html" >&2
      exit 1
    fi
  '';

  unit = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "dashboard";
    meta.mainProgram = "dashboard";
  };
in
unit.overrideAttrs {
  passthru = unit.passthru // {
    tests = unit.passthru.tests // {
      inherit site dashboardInSync;
    };
  };
}
