{
  id = "biff-todo-app";
  # Linux only, and the reason is in the lock rather than in our code:
  # deps-lock.json carries `brotli4j/native-linux-x86_64` and no
  # `native-osx-aarch64`, because clj-nix's generator resolved it on Linux and
  # brotli's natives are classifier-scoped. Requiring any Biff namespace that
  # loads Jetty then dies with "Failed to find Brotli native library:
  # /lib/osx-aarch64/libbrotli.dylib". Regenerating the lock on a Mac, or
  # pinning both classifiers, is what would lift this.
  # Reading List has no brotli dependency and builds everywhere.
  packageSet.systems = ["x86_64-linux"];
  flake.systems = ["x86_64-linux"];
  # service.nix became modules/services/biff-todo-app, which resolves the
  # application through `pkgs` rather than taking it as an argument. That is
  # what lets examples take only `index` (lib/discovery.nix skips the rest).
  overlay.systems = ["x86_64-linux"];
  # See biff-reading-list: runs the project's own suite as a flake check.
  # No `systems` here: passthruTests takes only `prefix`, and the package's
  # own x86_64-linux scoping above already keeps it off darwin.
  passthruTests.prefix = "clj-biff-todo-app";
}
