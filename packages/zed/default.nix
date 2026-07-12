{
  diffutils,
  ix,
  symlinkJoin,
  zedPackage,
  zedSource,
}: let
  patchedSource = ix.patchedSrc {
    name = "zed";
    src = ix.zedSrc;
    patchDir = ./patches;
  };
in
  symlinkJoin {
    inherit (zedPackage) pname version;
    paths = [zedPackage];
    postBuild = ''
      # shell
      ${diffutils}/bin/diff --recursive --brief --no-dereference ${patchedSource} ${zedSource}
    '';
    passthru = (zedPackage.passthru or {}) // {unwrapped = zedPackage;};
    meta =
      (zedPackage.meta or {})
      // {
        description = "Zed with index's reference-navigation patch";
        homepage = "https://github.com/zed-industries/zed";
        changelog = "https://zed.dev/releases/stable";
      };
  }
