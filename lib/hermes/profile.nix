# Service composition for the Hermes operator VM. The upstream NixOS
# module (`services.hermes-agent.*`) is included by `ix.nix`; this
# file only sets options on it.
#
# The `_module.args.hermes` surface keeps the on-disk options short for
# a downstream deployer: pick the integrations you want, point at the
# env file paths your secret-store wrote, ix-rebuild. Everything is
# outbound-only on this preset by default; the one inbound toggle is
# `apiServer`, which claims a port for the OpenAI-compatible
# `hermes api-server` (see examples/hermes/api-server).
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  hermes = config._module.args.hermes or {};

  # One env file holds every credential the daemon needs. Operators
  # using sops-nix or agenix can split it: each `*EnvFile` arg below
  # accepts a different path, and the module collapses duplicates.
  # `/run/secrets/hermes.env` is a stable in-VM path so the operator
  # workflow ("install -m0400 ... /run/secrets/hermes.env") is the same
  # whether the file lands from `ix shell` or from a secret manager.
  defaultEnvFile = hermes.envFile or "/run/secrets/hermes.env";
  telegramEnvFile = hermes.telegramEnvFile or defaultEnvFile;
  discordEnvFile = hermes.discordEnvFile or defaultEnvFile;
  webSearchEnvFile = hermes.webSearchEnvFile or defaultEnvFile;
  ttsEnvFile = hermes.ttsEnvFile or defaultEnvFile;
  homeAssistantEnvFile = hermes.homeAssistantEnvFile or defaultEnvFile;
  imageGenEnvFile = hermes.imageGenEnvFile or defaultEnvFile;
  memoryEnvFile = hermes.memoryEnvFile or defaultEnvFile;
  apiServerEnvFile = hermes.apiServerEnvFile or defaultEnvFile;

  # Feature toggles. All default off so a fresh fleet up boots the agent
  # with just the model provider key; turning anything else on is a one
  # line edit in the fleet preset that consumes this module.
  telegram = hermes.telegram or false;
  discord = hermes.discord or false;
  homeAssistant = hermes.homeAssistant or false;
  imageGen = hermes.imageGen or false;
  documents = hermes.documents or ix.hermes.documents.operator;

  # The OpenAI-compatible `hermes api-server` platform. Unlike every
  # other toggle this one is INBOUND: it claims a TCP port and opens the
  # in-guest firewall, so chat frontends (LobeChat, Open WebUI,
  # LibreChat) on sibling VMs can use this node as their model endpoint.
  # Reachability stays scoped by the fleet's east-west groups: a VM
  # outside the node's group has no route to the port. The
  # `examples/hermes/api-server` preset is the canonical consumer.
  apiServer = hermes.apiServer or false;
  apiServerPort = hermes.apiServerPort or 9119;

  # `null` leaves the corresponding integration off entirely. Strings
  # are validated against the known backend set; a typo fails the eval
  # with a named error instead of a runtime YAML reject.
  webSearch = hermes.webSearch or null;
  tts = hermes.tts or "edge";
  memory = hermes.memory or "holographic";

  # Model knobs default to OpenRouter routing to Claude Sonnet 4. Both
  # are overridable; pointing `modelBaseUrl` at api.anthropic.com or
  # api.openai.com plus changing the model string is the standard
  # provider swap. The single key the operator drops still lives in
  # `defaultEnvFile` (named OPENROUTER_API_KEY / ANTHROPIC_API_KEY /
  # OPENAI_API_KEY etc. matching the chosen base_url).
  modelDefault = hermes.modelDefault or "anthropic/claude-sonnet-4";
  modelBaseUrl = hermes.modelBaseUrl or "https://openrouter.ai/api/v1";

  webSearchBackends = [
    "tavily"
    "exa"
    "firecrawl"
    "parallel"
  ];
  ttsBackends = [
    "edge"
    "elevenlabs"
    "minimax"
    "openai"
  ];
  memoryBackends = [
    "holographic"
    "honcho"
    "openviking"
    "mem0"
    "hindsight"
    "retaindb"
    "byterover"
    "supermemory"
  ];

  validWebSearch =
    if webSearch == null
    then null
    else assert lib.assertOneOf "hermes.webSearch" webSearch webSearchBackends; webSearch;
  validTts = assert lib.assertOneOf "hermes.tts" tts ttsBackends; tts;
  validMemory = assert lib.assertOneOf "hermes.memory" memory memoryBackends; memory;

  envFiles = lib.unique (
    [defaultEnvFile]
    ++ lib.optional telegram telegramEnvFile
    ++ lib.optional discord discordEnvFile
    ++ lib.optional (validWebSearch != null) webSearchEnvFile
    ++ lib.optional (validTts != "edge") ttsEnvFile
    ++ lib.optional homeAssistant homeAssistantEnvFile
    ++ lib.optional imageGen imageGenEnvFile
    ++ lib.optional (validMemory != "holographic") memoryEnvFile
    ++ lib.optional apiServer apiServerEnvFile
  );
in {
  services.hermes-agent = {
    enable = true;

    # Put `hermes`, `hermes-agent`, and `hermes-acp` on PATH so
    # `ix shell hermes -- hermes chat` works without poking around for
    # the wrapped binary path.
    addToSystemPackages = true;

    # Native mode keeps the agent inside the same NixOS userland the
    # rest of the VM is built from. Container mode (Docker/Podman with
    # Ubuntu inside) is the upstream path for "agent wants apt"; on ix
    # the agent already has `nix shell nixpkgs#<tool>` and effectively
    # unbounded disk, so the container mode tradeoff does not pay off
    # here. Flip to true if a workload genuinely needs Ubuntu userland.
    container.enable = false;

    environmentFiles = envFiles;

    # The api-server platform is enabled through the gateway's env knobs
    # (the upstream YAML block is equivalent; env keeps this composable
    # with the toggle bag). Binding 0.0.0.0 is what makes the listener
    # reachable over the east-west network; the port is only routable
    # from VMs sharing a group with this node. API_SERVER_KEY is a
    # secret, so it stays in the env file, never here.
    environment = lib.optionalAttrs apiServer {
      API_SERVER_ENABLED = "true";
      API_SERVER_HOST = "0.0.0.0";
      API_SERVER_PORT = toString apiServerPort;
    };

    extraPackages = builtins.attrValues {
      inherit
        (pkgs)
        # Github CLI is the most common "the agent needs to read or
        # comment on a PR" tool. Other ecosystem-specific binaries
        # belong in deployer-side overrides via `extraPackages`.
        gh
        ;
    };

    # mkDefault so a sibling preset can swap the persona with a plain assignment.
    documents = lib.mapAttrs (_: lib.mkDefault) documents;

    settings =
      {
        model = {
          default = modelDefault;
          base_url = modelBaseUrl;
        };

        # `all` exposes the full upstream toolset (terminal, files, web,
        # vision, image, voice, delegation, memory). Narrow this per
        # deployment by listing toolset names if a node should be more
        # locked down.
        toolsets = ["all"];

        terminal = {
          backend = "local";
          cwd = ".";
          timeout = 180;
        };

        agent = {
          max_turns = 60;
          verbose = false;
        };

        memory = {
          provider = validMemory;
          memory_enabled = true;
          user_profile_enabled = true;
        };

        compression.enabled = true;
      }
      // lib.optionalAttrs (validWebSearch != null) {
        web.backend = validWebSearch;
      };

    # Filesystem MCP server pointed at the workspace so the agent can
    # read and write project files through a typed protocol instead of
    # raw shell commands. Hermes' upstream wraps Node into PATH so npx
    # resolves without extra wiring; the @modelcontextprotocol package
    # is fetched on first run.
    mcpServers.filesystem = {
      command = "npx";
      args = [
        "-y"
        "@modelcontextprotocol/server-filesystem"
        "/var/lib/hermes/workspace"
      ];
    };
  };

  # Surface the daemon health to fleet-wide health checks. The unit is
  # forking with `Restart=always`, so `is-active` is the right probe.
  ix.healthChecks.hermes-agent = {
    description = "Hermes agent gateway is active";
    unit = "hermes-agent";
  };

  # One source of truth for the api-server port: registers the port
  # claim (eval-time collision check), opens the in-guest firewall, and
  # makes the listener discoverable from sibling nodes via
  # `ix.endpointOf nodes.<node> "hermes-api"`.
  ix.networking.expose = lib.optionalAttrs apiServer {
    hermes-api = {
      port = apiServerPort;
      description = "Hermes OpenAI-compatible api-server";
    };
  };
}
