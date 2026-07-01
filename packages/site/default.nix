{
  ix,
  pkgs,
}:
let
  siteBuild = ix.buildSvelteSite pkgs {
    sourceRoot = ix.paths.packagesRoot + "/site";
    distDir = "build";
    serve.routePrefix = "/index";
  };
in
siteBuild.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    preview = siteBuild.passthru.serve;
    static = siteBuild.passthru.staticSite;
  };
})
