# JVM runtime profile.
#
# Opt-in: ships a JRE on PATH and sets `JAVA_HOME` so a VM that exists
# to run a `.jar` (Minecraft server, Velocity proxy, anything in the
# Minestom/Hyperion family) does not have to repeat the boilerplate in
# every service module. Build-time helpers (`ix.languages.java.{jdk,
# maven, gradle}`) stay separate — this profile is the runtime side.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.ix.profiles.jvm;
in
{
  options.ix.profiles.jvm = {
    enable = lib.mkEnableOption "Java runtime (JRE on PATH + JAVA_HOME)";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.temurin-jre-bin-25;
      defaultText = lib.literalExpression "pkgs.temurin-jre-bin-25";
      description = ''
        JRE package added to `environment.systemPackages` and pointed at by
        `JAVA_HOME`. Defaults to Temurin 25 because that is the binary the
        existing Minecraft, Minestom, and Velocity services already pin;
        keeping the profile default aligned means an image that turns on the
        profile and an image that hardcoded the same JRE end up with one
        store path in the closure instead of two.

        Override with another Temurin major or with the OpenJDK headless JDK
        when an image needs a TCK build or `javac` at runtime. For a jlink
        custom runtime, build it with `pkgs.jre_minimal` against the module
        list the service actually loads and pass the result here.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment = {
      systemPackages = [ cfg.package ];
      variables.JAVA_HOME = cfg.package.home or "${cfg.package}";
    };
  };
}
