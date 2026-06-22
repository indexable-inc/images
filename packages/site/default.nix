{
  ix,
  pkgs,
}:
let
  siteBuild = ix.buildSvelteSite pkgs {
    pname = "ix-site";
    version = "0.1.0";
    src = ix.paths.site;
    distDir = "build";
    serve = {
      name = "ix-site";
      routePrefix = "/index";
    };
    devServer = {
      name = "ix-site-dev";
      checkoutSubdir = "packages/site";
    };
  };
in
siteBuild.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    preview = siteBuild.passthru.serve;
    static = siteBuild.passthru.staticSite;
  };
})
