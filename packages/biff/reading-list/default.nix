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
    meta = {
      description = "Biff 2 reading list: form, effect, authorization, SQLite";
      mainProgram = "biff-reading-list";
    };
  }
