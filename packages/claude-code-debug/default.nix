# claude-code-debug: launch Claude Code with Bun's inspector bound so a JS
# debugger can attach to the LIVE process (breakpoints, scope variables, a REPL
# evaluated in the paused frame, heap snapshots).
#
# Claude Code is a `bun build --compile` standalone binary, but the embedded Bun
# runtime still honors the `BUN_INSPECT` env var: set it to `host:port/path` and
# the inspector binds a WebSocket once the runtime starts. The catch is that the
# TUI takes the alternate screen and swallows Bun's own "Inspect in browser: ..."
# banner (printed to stderr), so this wrapper prints the connect URL ITSELF
# before handing the terminal to the real binary.
{
  writeNushellApplication,
  repoPackages ? {},
}: let
  claude-code =
    repoPackages.claude-code
      or (throw "claude-code-debug: needs the claude-code sibling (flake package set only)");
  # The stock upstream binary as Anthropic shipped it (autopatchelfed on Linux),
  # exposed by claude-code as `passthru.stockCli`. Debugging the stock bytes
  # keeps the house wrapper out of the picture while inspecting internals. The
  # wrapped binary carries no byte patch at present (claude-code's dev-channels
  # gate swap is commented out), but that is the wrapper's choice to reverse, so
  # this stays pinned to the download rather than following it.
  claudeBin = "${claude-code.stockCli}/bin/claude";
in
  writeNushellApplication {
    name = "claude-debug";
    meta.description = "Launch Claude Code under the Bun inspector for live JS debugging";
    text = ''
      # nu
      def main [
        --host: string = "127.0.0.1" # inspector bind host
        --port: int = 9229           # inspector bind port
      ] {
        let ep = $"($host):($port)/claude"
        print "Bun inspector for Claude Code:"
        print $"  websocket: ws://($ep)"
        print $"  browser:   https://debug.bun.sh/#($ep)"
        print "  attach VS Code / Chrome DevTools to the websocket above"
        print ""
        # BUN_INSPECT binds the inspector; DISABLE_UPDATES keeps the store binary
        # from trying to self-update. The TUI inherits this terminal.
        with-env { BUN_INSPECT: $ep, DISABLE_UPDATES: "1" } {
          run-external "${claudeBin}"
        }
      }
    '';
  }
