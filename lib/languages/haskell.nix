{errors}: let
  /**
  GHC major → nixpkgs attribute mapping. nixpkgs lists the matrix at
  `pkgs.haskell.compiler.ghc<MM>`; the helper exposes the LTS-ish
  lines (9.6 / 9.8 / 9.10 / 9.12 / 9.14) that HLS still supports.
  `"latest"` follows `pkgs.ghc`, which is the channel default GHC
  that `cabal-install` and `stack` are built against.

  Bump the top entry when nixpkgs ships a newer stable GHC and HLS
  publishes a release that supports it; out-of-sync HLS + GHC pins
  silently degrade to no completions in the editor.
  */
  compilersFor = pkgs: {
    "latest" = pkgs.ghc;
    "9.6" = pkgs.haskell.compiler.ghc96;
    "9.8" = pkgs.haskell.compiler.ghc98;
    "9.10" = pkgs.haskell.compiler.ghc910;
    "9.12" = pkgs.haskell.compiler.ghc912;
    "9.14" = pkgs.haskell.compiler.ghc914;
  };
in {
  /**
  Return the GHC compiler for `version`.

  "compiler" matches the Haskell ecosystem's own term: `ghc` is the
  compiler binary, `runghc` runs a source file under the same
  front end, and `cabal` / `stack` are project orchestrators on top.

  Bare `pkgs.ghc` ships with the standard `base`, `containers`, and
  `bytestring` packages built in; everything else comes through
  `cabal-install`. For a sealed runtime image that bundles a
  pre-compiled binary, the GHC `runtime` (`pkgs.ghc.runtimeShared`)
  is enough; reach for the full compiler here when the VM has to
  build or run sources in place.

  Arguments:
  - `pkgs`: nixpkgs instance the compiler comes from.
  - `version`: required, one of `"latest" | "9.6" | "9.8" | "9.10" |
    "9.12" | "9.14"`. Pass `"latest"` to follow `pkgs.ghc`; pin a
    specific minor when an upstream library has a known
    incompatibility with a newer GHC (typeclass-resolution changes
    between 9.x lines are still a regular source of build breakage).

  Example:
  ```nix
  { pkgs, ix, ... }:
  let ghc = ix.languages.haskell.compiler pkgs { version = "9.10"; };
  in { environment.systemPackages = [ ghc ]; }
  ```
  */
  compiler = pkgs: args: let
    version = errors.requireArg {
      context = "ix.languages.haskell.compiler";
      inherit args;
      name = "version";
    };
  in
    errors.requireAttr {
      context = "ix.languages.haskell.compiler: unknown version";
      attrset = compilersFor pkgs;
      key = version;
    };

  /**
  Return cabal-install.

  The default Haskell project tool: reads `cabal.project` / `*.cabal`,
  resolves dependencies against Hackage, and shells out to GHC. The
  floating nixpkgs `cabal-install` works against any of the GHC
  versions above; override only if a project pins a cabal feature
  older releases lack.
  */
  cabal = pkgs: _: pkgs.cabal-install;

  /**
  Return Stack.

  Alternative to cabal-install; resolves against Stackage curated
  snapshots rather than Hackage `cabal.project.freeze` files.
  Relevant when an upstream project commits a `stack.yaml` and the
  snapshot is what guarantees the dependency closure.
  */
  stack = pkgs: _: pkgs.stack;

  /**
  Return the Haskell language server package.

  HLS is GHC-version-sensitive: a `pkgs.haskell-language-server` build
  targets one specific GHC, and an editor pointed at the wrong pair
  silently produces no completions. The floating default here matches
  `pkgs.ghc`; if `compiler` is pinned to a non-default GHC the caller
  should rebuild HLS against that GHC via
  `pkgs.haskell-language-server.override { supportedGhcVersions = [ "..." ]; }`.
  */
  languageServer = pkgs: _: pkgs.haskell-language-server;
}
