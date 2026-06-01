{ errors }:
let
  defaultJvmVersion = import ./jvm-defaults.nix;

  validVersions = [
    "2"
    "3"
  ];

  /**
    Scala major → nixpkgs attribute mapping. `pkgs.scala` is the Scala 2
    line (currently 2.13), `pkgs.scala_3` is the Dotty-based Scala 3.
    The split matters: 2.x and 3.x are different compilers with
    overlapping but non-identical syntax, and a build that pins one will
    not transparently switch to the other.
  */
  compilersFor = pkgs: {
    "2" = pkgs.scala;
    "3" = pkgs.scala_3;
  };

  /**
    Default JDK paired with Scala 2/3 when the caller does not pass
    `jdk`. Tracks the major pinned in
    [`./jvm-defaults.nix`](./jvm-defaults.nix), shared with
    `ix.languages.java`'s `defaultJdkFor` and `ix.profiles.jvm`, so a
    service that resolves the Scala compiler and a Java runtime
    without overriding pulls one JDK into the closure rather than two.
  */
  defaultJdkFor = pkgs: pkgs."jdk${defaultJvmVersion}_headless";
in
{
  /**
    Return the Scala compiler for the requested major, overridden to use
    the chosen JDK.

    Scala compiles to JVM bytecode and runs on a host JVM, so the
    compiler and the runtime JDK need to agree. Routing the override
    through this helper keeps `scalac`'s `--target` and the runtime JVM
    on the same major instead of letting `pkgs.scala.jre` float.

    Arguments:
    - `pkgs`: nixpkgs instance the compiler and JDK come from.
    - `version`: required, Scala major (`"2" | "3"`). Pick `"2"` only
      when an upstream library has not migrated; the long-term
      destination is `"3"`.
    - `jdk`: optional resolved JDK. Defaults to the OpenJDK headless
      major pinned in [`./jvm-defaults.nix`](./jvm-defaults.nix).

    Example:
    ```nix
    { pkgs, ix, ... }:
    let
      jdk = ix.languages.java.jdk pkgs { version = "21"; distribution = "openjdk"; };
      scala = ix.languages.scala.compiler pkgs { version = "3"; inherit jdk; };
    in {
      environment = {
        systemPackages = [ jdk scala ];
        variables.JAVA_HOME = jdk.home;
      };
    }
    ```
  */
  compiler =
    pkgs:
    args@{
      jdk ? defaultJdkFor pkgs,
      ...
    }:
    let
      version = errors.requireArg {
        context = "ix.languages.scala.compiler";
        inherit args;
        name = "version";
      };
      checkedVersion = errors.assertEnum {
        name = "ix.languages.scala.compiler.version";
        value = version;
        valid = validVersions;
      };

      compilerPackage = errors.requireAttr {
        context = "ix.languages.scala.compiler: unknown major";
        attrset = compilersFor pkgs;
        key = checkedVersion;
      };
    in
    compilerPackage.override { jre = jdk; };

  /**
    Return the sbt build tool, overridden to run on the chosen JDK.

    sbt drives most Scala 2 projects and a non-trivial fraction of
    Scala 3 ones. Pinning the JRE it launches under prevents the build
    daemon from reading project metadata with a JDK other than the one
    `scalac` targets.
  */
  sbt =
    pkgs:
    {
      jdk ? defaultJdkFor pkgs,
    }:
    pkgs.sbt.override { jre = jdk; };

  /**
    Return the mill build tool, overridden to run on the chosen JDK.

    Mill is the Bazel-influenced alternative to sbt; faster cold builds
    and a Scala-only build DSL. Same JRE-pinning rationale as `sbt`.
  */
  mill =
    pkgs:
    {
      jdk ? defaultJdkFor pkgs,
    }:
    pkgs.mill.override { jre = jdk; };

  /**
    Return the Metals language server, overridden to run on the chosen
    JDK. Intended for dev VMs that host an editor; runtime-only servers
    that just execute compiled `.jar` artifacts do not need it.
  */
  languageServer =
    pkgs:
    {
      jdk ? defaultJdkFor pkgs,
    }:
    pkgs.metals.override { jre = jdk; };
}
