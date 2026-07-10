{
  ix,
  pkgs,
  ...
}: let
  L = ix.languages;

  # One JDK threaded through every JVM tool below so Java, Maven, Gradle,
  # Kotlin's stdlib runtime, Scala, sbt, mill, and Metals all share one
  # store path instead of pinning their own. Temurin 25 because it's the
  # current LTS (released Sep 2025) and it matches what every existing
  # JVM service module and `ix.profiles.jvm` default to, so the OCI
  # closure does not pick up a second JDK store path by accident.
  jdk = L.java.jdk pkgs {
    version = "25";
    distribution = "temurin";
  };

  jvm = {
    inherit jdk;
    kotlin = L.kotlin.compiler pkgs {target = "jvm";};
    scala = L.scala.compiler pkgs {
      version = "3";
      inherit jdk;
    };
    maven = L.java.maven pkgs {inherit jdk;};
    gradle = L.java.gradle pkgs {
      version = "9";
      inherit jdk;
    };
  };

  native = {
    # Stable rust here rather than the repo-default nightly: this VM is
    # for human iteration, not the repo's own clippy policy pipeline,
    # and `cargo` + `clippy` + `rustfmt` against stable removes the
    # nightly-feature foot-cannon.
    rust = L.rust.toolchain pkgs {
      channel = "stable";
      version = "latest";
      components = [
        "cargo"
        "clippy"
        "rust-src"
        "rust-std"
        "rustc"
        "rustfmt"
      ];
    };
    cpp = L.cpp.compiler pkgs {
      vendor = "gcc";
      version = "latest";
    };
    cmake = L.cpp.cmake pkgs {};
    ninja = L.cpp.ninja pkgs {};
    zig = L.zig.toolchain pkgs {version = "latest";};
  };

  scripting = {
    python = L.python.interpreter pkgs {version = "3.14";};
    go = L.go.toolchain pkgs {version = "latest";};
    node = L.javascript.node pkgs {version = "24";};
    bun = L.javascript.bun pkgs {};
    deno = L.javascript.deno pkgs {};
    typescript = L.javascript.typescript pkgs {};
  };

  functional = {
    haskell = L.haskell.compiler pkgs {version = "latest";};
    cabal = L.haskell.cabal pkgs {};
    ocaml = L.ocaml.compiler pkgs {version = "latest";};
    dune = L.ocaml.dune pkgs {version = "latest";};
  };

  beam = {
    erlang = L.erlang.toolchain pkgs {version = "latest";};
    elixir = L.elixir.toolchain pkgs {version = "latest";};
    # Gleam is the BEAM-targeting statically-typed cousin whose compiler
    # itself is written in Rust; pairs with `erlang` above for the runtime.
    gleam = L.gleam.compiler pkgs {};
  };

  # Language servers stay together so an editor inside the VM finds the
  # whole set in one PATH lookup. `scala.languageServer` (Metals) needs
  # the same JDK the compiler was overridden against so loaded sources
  # parse against one runtime; `ocaml.languageServer` is
  # compiler-version-coupled so it takes the same OCaml version as the
  # `compiler` above; the rest are JDK- and version-independent.
  languageServers = [
    (L.cpp.languageServer pkgs {})
    (L.go.languageServer pkgs {})
    (L.haskell.languageServer pkgs {})
    (L.java.languageServer pkgs {})
    (L.javascript.languageServer pkgs {})
    (L.kotlin.languageServer pkgs {})
    (L.ocaml.languageServer pkgs {version = "latest";})
    (L.scala.languageServer pkgs {inherit jdk;})
    (L.zig.languageServer pkgs {})
  ];
in {
  # The runtime JVM profile sets JAVA_HOME from the package it gets, so
  # passing the same `jdk` here makes `java -version`, `javac`, and
  # every JAR launched without an explicit `-Djava.home=...` line up
  # with the one Kotlin/Scala/Maven/Gradle were configured against.
  ix.profiles.jvm = {
    enable = true;
    package = jdk;
  };

  environment.systemPackages =
    (builtins.attrValues (builtins.removeAttrs jvm ["jdk"]))
    ++ builtins.attrValues native
    ++ builtins.attrValues scripting
    ++ builtins.attrValues functional
    ++ builtins.attrValues beam
    ++ languageServers;
}
