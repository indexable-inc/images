{
  symlinkJoin,
  zedPackage,
}:
# zed-src is the maintained jj megamerge fork (indexable-inc/zed, bookmark
# ix-patched) consumed as a flake, so the fork repo is the only serialization
# of the delta; the old upstream+patches equality diff died with the in-repo
# series. This wrapper only owns index's meta.
symlinkJoin {
  inherit (zedPackage) pname version;
  paths = [zedPackage];
  passthru = (zedPackage.passthru or {}) // {unwrapped = zedPackage;};
  meta =
    (zedPackage.meta or {})
    // {
      description = "Zed with index's reference-navigation patch";
      homepage = "https://github.com/zed-industries/zed";
      changelog = "https://zed.dev/releases/stable";
    };
}
