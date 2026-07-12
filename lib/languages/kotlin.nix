{errors}: let
  validTargets = [
    "jvm"
    "native"
  ];

  /**
  Target → nixpkgs attribute that ships the matching Kotlin compiler.
  `pkgs.kotlin` is Kotlin/JVM (`kotlinc` to JVM bytecode), `pkgs.kotlin-native`
  is Kotlin/Native (`kotlinc-native` to a standalone executable, no JVM at
  runtime). Listed explicitly so an unknown target fails with the supported
  set rather than `attribute missing`.
  */
  compilersFor = pkgs: {
    jvm = pkgs.kotlin;
    native = pkgs.kotlin-native;
  };
in {
  /**
  Return the Kotlin compiler package for the requested target.

  Kotlin/JVM is the common case in this repo (Minecraft plugins, JAR-shaped
  services that share a JVM with Java upstreams). Kotlin/Native is for
  standalone binaries with no JVM at runtime; it is a different toolchain,
  not a flag on `kotlinc`, so the choice is committed to at package
  selection rather than at invocation time.

  Pair with [`ix.languages.java.jdk`](./java/default.nix) for the runtime
  JDK on the `jvm` target; Kotlin/Native produces a self-contained
  executable and does not need one.

  Arguments:
  - `pkgs`: nixpkgs instance the compiler comes from.
  - `target`: required, one of `"jvm" | "native"`.

  Example:
  ```nix
  { pkgs, ix, ... }:
  let
    jdk = ix.languages.java.jdk pkgs { version = "21"; distribution = "openjdk"; };
    kotlinc = ix.languages.kotlin.compiler pkgs { target = "jvm"; };
  in {
    environment = {
      systemPackages = [ jdk kotlinc ];
      variables.JAVA_HOME = jdk.home;
    };
  }
  ```
  */
  compiler = pkgs: args: let
    target = errors.requireArg {
      context = "ix.languages.kotlin.compiler";
      inherit args;
      name = "target";
    };
    checkedTarget = errors.assertEnum {
      name = "ix.languages.kotlin.compiler.target";
      value = target;
      valid = validTargets;
    };
  in
    errors.requireAttr {
      context = "ix.languages.kotlin.compiler: unknown target";
      attrset = compilersFor pkgs;
      key = checkedTarget;
    };

  /**
  Return the Kotlin language server package.

  Intended for dev VMs that host an editor (the YourKit-attached
  workstation pattern, remote-desktop images). Runtime-only server
  images that just execute compiled `.jar` artifacts do not need it.
  */
  languageServer = pkgs: _: pkgs.kotlin-language-server;

  /**
  Return the Kotlin interactive shell (`ki`, the official REPL/scratch
  runner) for ad-hoc evaluation inside a VM.
  */
  repl = pkgs: _: pkgs.kotlin-interactive-shell;
}
