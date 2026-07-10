{errors}: let
  /**
  Mapping from human version strings (`"3.12"`) to the matching
  nixpkgs interpreter attribute. Listed explicitly so the error from
  an unknown version names the supported set; bump the top entry when
  the platform follows nixpkgs onto a newer Python.
  */
  interpretersFor = pkgs: {
    "3.10" = pkgs.python310;
    "3.11" = pkgs.python311;
    "3.12" = pkgs.python312;
    "3.13" = pkgs.python313;
    "3.14" = pkgs.python314;
  };
in {
  /**
  Return the nixpkgs Python interpreter for `version`.

  `version` is a `major.minor` string matching one of the supported
  entries; unknown versions throw with the supported set listed so a
  typo or a too-new pin is fixable from the error message alone.

  `version` is required: a floating default would mean an interpreter
  bump in nixpkgs silently retargets every consumer. Pass `"3.14"` to
  match `writePythonApplication` and `buildUvApplication`.

  Arguments:
  - `pkgs`: nixpkgs instance to look the interpreter up in. Modules pass
    their own `pkgs` so the returned package is from the image's
    evaluation rather than the lib's default.
  - `version`: required `major.minor` string.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let python = ix.languages.python.interpreter pkgs { version = "3.12"; };
  in { environment.systemPackages = [ python ]; }
  ```
  */
  interpreter = pkgs: args: let
    version = errors.requireArg {
      context = "ix.languages.python.interpreter";
      inherit args;
      name = "version";
    };
  in
    errors.requireAttr {
      context = "ix.languages.python.interpreter: unknown version";
      attrset = interpretersFor pkgs;
      key = version;
    };
}
