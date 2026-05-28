{ lib }:
let
  reservedSegments = [
    ""
    "."
    ".."
  ];
  segments = path: lib.splitString "/" path;
  hasReservedSegment =
    path: lib.any (segment: builtins.elem segment reservedSegments) (segments path);
  isSafe =
    path:
    builtins.isString path && path != "" && !(lib.hasPrefix "/" path) && !(hasReservedSegment path);
  isSafeName = path: isSafe path && builtins.length (segments path) == 1;
  assertSafe =
    path:
    assert lib.assertMsg (isSafe path) (
      "ix.relativePath.shellPath expected a safe relative path, got "
      + (if builtins.isString path then path else "<${builtins.typeOf path}>")
    );
    path;
  shellPath = root: path: ''"${root}"/${lib.escapeShellArg (assertSafe path)}'';
  shellParent =
    root: path:
    let
      parent = dirOf (assertSafe path);
    in
    if parent == "." then ''"${root}"'' else shellPath root parent;
in
{
  inherit
    isSafe
    isSafeName
    shellParent
    shellPath
    ;
  unsafe = paths: lib.filter (path: !(isSafe path)) paths;
  unsafeNames = paths: lib.filter (path: !(isSafeName path)) paths;
}
