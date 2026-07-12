# API-server deltas on top of the shared hermes-agent composition
# (`ix.hermes.profile`). The `apiServer` toggle does the heavy
# lifting there: it sets the gateway's API_SERVER_* env knobs, claims
# the TCP port through `ix.networking.expose.hermes-api` (eval-time
# port collision check + in-guest firewall), and wires the env file
# that carries API_SERVER_KEY into the daemon.
let
  # 9119 is this example's contract with its consumers (the README's
  # LobeChat/Open WebUI snippets). Stated here so the preset, not a
  # default buried in the shared composition, owns the number.
  port = 9119;
in {
  _module.args.hermes = {
    apiServer = true;
    apiServerPort = port;
  };

  # The api-server is only useful if something can reach it: assert the
  # exposed listener answers HTTP from inside the guest, which proves
  # the gateway actually bound 0.0.0.0:${toString port} (and not just
  # that the unit is active, which the shared composition already
  # checks). Any HTTP status counts: with API_SERVER_KEY set an
  # unauthenticated /v1/models is a 401, which is still a live
  # listener; a connection refused is a curl exit 7 and a failed check.
  ix.healthChecks.hermes-api = {
    description = "Hermes api-server answers on its exposed port";
    command = [
      "curl"
      "--silent"
      "--max-time"
      "10"
      "--output"
      "/dev/null"
      "http://127.0.0.1:${toString port}/v1/models"
    ];
  };
}
