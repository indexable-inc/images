{
  errors,
  lib,
}:
let
  defaultJvmVersion = import ../jvm-defaults.nix;

  validDistributions = [
    "openjdk"
    "temurin"
    "corretto"
    "zulu"
  ];

  /**
    Per-distribution version → package mapping. Listed explicitly so an
    unknown version throws with the supported set for the chosen
    distribution rather than a confusing `attribute missing` deep in
    eval. The covered versions match the LTS lines nixpkgs ships
    headless variants for; bump the tables when a new LTS or platform
    target needs it.
  */
  jdksFor = pkgs: {
    openjdk = {
      "8" = pkgs.jdk8_headless;
      "11" = pkgs.jdk11_headless;
      "17" = pkgs.jdk17_headless;
      "21" = pkgs.jdk21_headless;
      "23" = pkgs.jdk23_headless;
      "24" = pkgs.jdk24_headless;
      "25" = pkgs.jdk25_headless;
    };
    temurin = {
      "8" = pkgs.temurin-bin-8;
      "11" = pkgs.temurin-bin-11;
      "17" = pkgs.temurin-bin-17;
      "21" = pkgs.temurin-bin-21;
      "23" = pkgs.temurin-bin-23;
      "24" = pkgs.temurin-bin-24;
      "25" = pkgs.temurin-bin-25;
    };
    corretto = {
      "11" = pkgs.corretto11;
      "17" = pkgs.corretto17;
      "21" = pkgs.corretto21;
      "25" = pkgs.corretto25;
    };
    zulu = {
      "8" = pkgs.zulu8;
      "11" = pkgs.zulu11;
      "17" = pkgs.zulu17;
      "21" = pkgs.zulu21;
      "23" = pkgs.zulu23;
      "24" = pkgs.zulu24;
      "25" = pkgs.zulu25;
    };
  };

  /**
    Resolve the JDK package this namespace defaults to when a sibling
    helper (`maven`, `gradle`) does not get an explicit `jdk` argument.
    Pulls from the same table as the `jdk` helper so a caller that
    overrides nothing gets one consistent JDK across the toolchain.

    Tracks the version pinned in [`../jvm-defaults.nix`](../jvm-defaults.nix),
    which `ix.languages.scala`, `ix.profiles.jvm`, and the JVM service
    modules also read. An image resolving `maven` and `gradle` without
    overriding shares that one store path with the runtime JRE instead
    of pulling a second JDK closure. Pass `jdk = ...` explicitly when a
    tool needs a different runtime.
  */
  defaultJdkFor = pkgs: (jdksFor pkgs).openjdk.${defaultJvmVersion};

  /**
    Per-major-version Gradle attribute mapping. nixpkgs also exposes a
    floating `pkgs.gradle` alias, but the explicit attributes make the
    resolver diff reviewable when a Gradle major moves.
  */
  gradlesFor = pkgs: {
    "7" = pkgs.gradle_7;
    "8" = pkgs.gradle_8;
    "9" = pkgs.gradle_9;
  };
in
{
  /**
    Return a JDK package for the requested version and distribution.

    Unknown distributions and unknown versions for a distribution throw
    with the supported set listed, so a typo (`"openjdkk"`, `"22"` when
    only `21` and `23` ship) is fixable from the message alone.

    OpenJDK uses the `_headless` variant by default so a server image
    that only needs the JDK does not pull X11 and CUPS into the closure.
    Pull a different distribution (Temurin, Corretto, Zulu) when an
    upstream needs a specific TCK-certified build.

    Arguments:
    - `pkgs`: nixpkgs instance the JDK comes from.
    - `version`: required, major version as a string (`"8" | "11" |
      "17" | "21" | "23" | "24" | "25"`). `"21"` is the current
      long-term-support line.
    - `distribution`: required, one of `"openjdk" | "temurin" |
      "corretto" | "zulu"`.

    Example:
    ```nix
    { pkgs, ix, ... }:
    let
      jdk = ix.languages.java.jdk pkgs {
        version = "21";
        distribution = "temurin";
      };
    in {
      environment = {
        systemPackages = [ jdk ];
        variables.JAVA_HOME = jdk.home;
      };
    }
    ```
  */
  jdk =
    pkgs: args:
    let
      version = errors.requireArg {
        context = "ix.languages.java.jdk";
        inherit args;
        name = "version";
      };
      distribution = errors.requireArg {
        context = "ix.languages.java.jdk";
        inherit args;
        name = "distribution";
      };

      checkedDistribution = errors.assertEnum {
        name = "ix.languages.java.jdk.distribution";
        value = distribution;
        valid = validDistributions;
      };

      jdkTable = errors.requireAttr {
        context = "ix.languages.java.jdk: distribution table";
        attrset = jdksFor pkgs;
        key = checkedDistribution;
      };
    in
    errors.requireAttr {
      context = "ix.languages.java.jdk: unknown version for distribution '${checkedDistribution}'";
      attrset = jdkTable;
      key = version;
    };

  /**
    Return a Maven package overridden to use the chosen JDK.

    `pkgs.maven` defaults to whatever JDK the nixpkgs Maven derivation pins,
    which floats with channel updates. Routing it through the
    `ix.languages.java.jdk` selection keeps Maven, the toolchain it launches,
    and the runtime JRE on one version, so a `mvn package` cannot silently
    target a different bytecode level than the deploy image.

    Arguments:
    - `pkgs`: nixpkgs instance the Maven and JDK packages come from.
    - `jdk`: optional resolved JDK package. Defaults to the OpenJDK
      headless major pinned in [`../jvm-defaults.nix`](../jvm-defaults.nix), matching `ix.profiles.jvm` and the rest of this
      namespace.

    Example:
    ```nix
    { pkgs, ix, ... }:
    let
      jdk = ix.languages.java.jdk pkgs { version = "21"; distribution = "temurin"; };
      maven = ix.languages.java.maven pkgs { inherit jdk; };
    in {
      environment = {
        systemPackages = [ jdk maven ];
        variables.JAVA_HOME = jdk.home;
      };
    }
    ```
  */
  maven =
    pkgs:
    {
      jdk ? defaultJdkFor pkgs,
    }:
    pkgs.maven.override { jdk_headless = jdk; };

  /**
    Return a Gradle package on the requested major version, overridden to
    use the chosen JDK.

    Gradle's compatibility matrix is the load-bearing thing here: Gradle 7
    refuses JDK 21+, Gradle 8 added 21 mid-line, Gradle 9 dropped support
    for JDK 8 and 11 daemons. Picking the major explicitly and pinning the
    JDK underneath keeps that matrix legible at the call site instead of
    drifting through `pkgs.gradle` channel bumps.

    Arguments:
    - `pkgs`: nixpkgs instance the Gradle and JDK packages come from.
    - `jdk`: optional resolved JDK package. Defaults to the OpenJDK
      headless major pinned in [`../jvm-defaults.nix`](../jvm-defaults.nix), the same JDK every other helper in this namespace
      assumes.
    - `version`: required, Gradle major as a string (`"7" | "8" |
      "9"`). `"9"` matches `lib/build/gradle-fat-jar.nix`.
  */
  gradle =
    pkgs:
    args@{
      jdk ? defaultJdkFor pkgs,
      ...
    }:
    let
      version = errors.requireArg {
        context = "ix.languages.java.gradle";
        inherit args;
        name = "version";
      };
      gradlePackage = errors.requireAttr {
        context = "ix.languages.java.gradle: unknown Gradle major";
        attrset = gradlesFor pkgs;
        key = version;
      };
    in
    gradlePackage.override { java = jdk; };

  /**
    Return the Eclipse JDT language server package.

    Intended for dev VMs that host an editor (remote-desktop image, an
    in-VM neovim/vscode workflow). Runtime-only server images that just
    execute compiled `.jar` artifacts do not need it.
  */
  languageServer = pkgs: _: pkgs.jdt-language-server;

  /**
    YourKit profiler integration for JVM services.

    See [`./yourkit.nix`](./yourkit.nix) for the option submodule plus the
    `flagsFor` / `portClaimFor` helpers that a service module pulls into
    its JVM args and firewall config when the option is enabled. Defaults
    are off; when enabled the agent loads at JVM startup so the first
    instruction is profiled (matches YourKit's startup-attach docs).
  */
  yourkit = import ./yourkit.nix { inherit errors lib; };
}
