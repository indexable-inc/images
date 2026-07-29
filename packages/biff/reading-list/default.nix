# Biff 2 reading list: the smallest complete form -> effect -> authorization
# -> SQLite path, in one namespace.
#
# Built as one content-addressed derivation per Clojure namespace
# ([`lib/build/clj-unit.nix`](../../../lib/build/clj-unit.nix)) over a
# dependency closure of one fetch derivation per artifact
# ([`lib/build/clj-lock.nix`](../../../lib/build/clj-lock.nix)). The
# deployment lives in `modules/services/biff-reading-list`; the `ix apply`
# demo lives in `examples/biff/reading-list`.
{
  ix,
  lib,
  pkgs,
  ...
}: let
  # Scoped source: the README and the clj-kondo config are not build inputs,
  # so editing them must not re-hash the package.
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./src
      ./deps.edn
    ];
  };
in
  ix.cljUnit.buildApplication {
    pname = "biff-reading-list";
    version = "0.1.0";
    inherit src;
    mainNamespace = "com.example.reading-list";
    sourceRoots = ["src"];
    # `:paths` in deps.edn names "resources", but the directory does not
    # exist: this example keeps its schema in the service's state directory.
    resourceRoots = [];
    classpathJars = ix.cljLock.classpathFor {lock = ./deps-lock.json;};
    # The `:test` alias. Kept out of `src` so editing a test does not re-run
    # the namespace-graph render. sqldef is a runtime input because the
    # application shells out to it for migrations, and downloads it when it
    # is absent (there is no darwin arm64 release, so absence is a failure,
    # not a slow path).
    testSrc = lib.fileset.toSource {
      root = ./.;
      fileset = ./test;
    };
    testNamespace = "com.example.reading-list-test";
    testInputs = [pkgs.sqldef];
    meta = {
      description = "Biff 2 reading list: form, effect, authorization, SQLite";
      mainProgram = "biff-reading-list";
    };
  }
