{
  errors,
  lib,
}: let
  inherit (lib) mkOption types;

  validListens = [
    "all"
    "localhost"
  ];

  /**
  Default port the YourKit UI connects to. Matches YourKit's documented
  default so an operator pointing the desktop UI at the VM does not
  have to override anything on the first try.

  Refs: https://www.yourkit.com/docs/java/help/startup_options.jsp
  */
  defaultPort = 10001;

  /**
  Resolve the YourKit agent's `libyjpagent` path for the host the
  target JVM runs on. nixpkgs lays the package out as a YourKit-style
  installation tree (`bin/<platform>/libyjpagent.so`), so picking the
  right subdirectory is a function of the runtime platform rather
  than something the caller needs to know.
  */
  agentSubdirFor = pkgs: let
    inherit
      (pkgs.stdenv.hostPlatform)
      isLinux
      isDarwin
      isAarch64
      isx86_64
      ;
  in
    if isLinux && isx86_64
    then "bin/linux-x86-64/libyjpagent.so"
    else if isLinux && isAarch64
    then "bin/linux-arm-64/libyjpagent.so"
    else if isDarwin
    then "Contents/Resources/bin/mac/libyjpagent.dylib"
    else throw "ix.languages.java.yourkit: no libyjpagent layout known for ${pkgs.stdenv.hostPlatform.system}";
in {
  /**
  Default port + listen mode + listen-on-all-interfaces flag the
  options below pick up. Exposed so callers that want to derive
  other config (a service description, an external port-claim, a
  docs page) can reuse the same values.
  */
  defaults = {
    inherit defaultPort;
    listen = "localhost";
  };

  /**
  Submodule type for a `yourkit = { ... }` option on a JVM service.

  Plug it in like this:

  ```nix
  options.services.foo.yourkit = lib.mkOption {
    type = ix.languages.java.yourkit.type;
    default = { };
    description = "YourKit profiler agent. Enable to load libyjpagent at JVM startup.";
  };
  ```

  Then in the service's `config` block, splice
  `ix.languages.java.yourkit.flagsFor pkgs cfg.yourkit` into the
  JVM args list and `ix.languages.java.yourkit.portClaimFor { owner = "foo"; cfg = cfg.yourkit; }`
  into `ix.networking.portClaims`. The submodule already validates
  `listen` and clamps `port` to a `types.port`, so a typo fails at eval.
  */
  type = types.submodule {
    options = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Load the YourKit profiler agent at JVM startup via
          `-agentpath`. Off by default because the agent is unfree
          (`pkgs.yourkit-java`) and most operators do not want a
          profiler endpoint listening in production.
        '';
      };

      package = mkOption {
        type = types.nullOr types.package;
        default = null;
        defaultText = lib.literalExpression "pkgs.yourkit-java";
        description = ''
          YourKit installation tree containing `libyjpagent`. When
          unset, `flagsFor` resolves `pkgs.yourkit-java` for the
          target JVM's package set; override to pin a specific build or
          point at a locally-licensed install.
        '';
      };

      port = mkOption {
        type = types.port;
        default = defaultPort;
        description = ''
          Port the agent listens on for the YourKit UI. Matches
          YourKit's documented default `10001`.
        '';
      };

      listen = mkOption {
        type = types.enum validListens;
        default = "localhost";
        description = ''
          `localhost` keeps the agent bound to the loopback interface
          (recommended; reach it over an `ix shell` SSH tunnel).
          `all` exposes it on every interface, which only makes sense
          on a trusted network and with `openFirewall = true`.
        '';
      };

      openFirewall = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Whether to open `port` in the in-image firewall. Off by
          default to keep the profiler endpoint invisible until the
          operator opts in.
        '';
      };

      sessionName = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "minecraft-prod";
        description = ''
          Optional `sessionname=` agent option. Shows up in the YourKit
          UI's session picker, which is useful when several JVMs on a
          fleet report into the same workstation.
        '';
      };

      extraOptions = mkOption {
        type = types.listOf types.str;
        default = [];
        example = [
          "sampling"
          "allocsamplinginterval=10"
        ];
        description = ''
          Additional `key=value` options appended to the agent string.
          Every entry is joined with `,` and passed straight to
          `libyjpagent`; see YourKit's startup-options reference for
          the full surface.

          Refs: https://www.yourkit.com/docs/java/help/startup_options.jsp
        '';
      };
    };
  };

  /**
  Build the `-agentpath:<libyjpagent>=<options>` flag(s) the JVM
  needs to load YourKit at startup.

  Returns an empty list when `cfg.enable` is `false` so the caller
  can splice the result unconditionally into its `jvmFlags`. When
  enabled, exactly one `-agentpath` flag is emitted (more than one
  YourKit agent in a single JVM is not supported by YourKit).
  */
  flagsFor = pkgs: cfg: let
    yourkitPackage =
      if cfg.package == null
      then pkgs.yourkit-java
      else cfg.package;
    libyjpagent = "${yourkitPackage}/${agentSubdirFor pkgs}";
    base =
      [
        "port=${toString cfg.port}"
      ]
      ++ lib.optional (cfg.listen == "all") "listen=all"
      ++ lib.optional (cfg.sessionName != null) "sessionname=${cfg.sessionName}"
      ++ cfg.extraOptions;
    options = lib.concatStringsSep "," base;
  in
    lib.optional cfg.enable "-agentpath:${libyjpagent}=${options}";

  /**
  Port-claim entry to merge into `ix.networking.portClaims`. The
  `owner` argument is the service name (`"minecraft"`, `"minestom"`,
  ...) so claims from different services co-exist in one namespace
  without colliding.
  */
  portClaimFor = {
    owner,
    cfg,
  }:
    lib.optionalAttrs cfg.enable {
      "${owner}-yourkit" = {
        protocol = "tcp";
        inherit (cfg) port;
        description = "YourKit profiler agent (${owner})";
      };
    };

  /**
  Convenience: the set of firewall TCP ports the option emits.
  Wraps the `openFirewall` + `enable` interaction so a service does
  not have to repeat the conjunction.
  */
  firewallTcpPortsFor = cfg: lib.optional (cfg.enable && cfg.openFirewall) cfg.port;

  /**
  Validate a free-form listen value via `errors.assertEnum`. Useful
  when a service surfaces its own front door to the option (a
  fleet-level helper, an example flake) and wants the same error
  shape the type already enforces inside an image.
  */
  assertListen = value:
    errors.assertEnum {
      name = "ix.languages.java.yourkit.listen";
      inherit value;
      valid = validListens;
    };
}
