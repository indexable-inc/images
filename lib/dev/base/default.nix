# Default ix environment base: agent CLIs plus the dev-only build extras.
# The auto-enabled base profile supplies version control, editors, the nushell
# workspace wrapper, debuggers, tracing tools, archive utilities, the default
# language runtimes (python3, node, uv), and the C toolchain (gcc, make).
{
  ix,
  pkgs,
  ...
}: {
  imports = [(ix.paths.root + "/lib/dev/agents.nix")];

  environment.systemPackages = builtins.attrValues {
    inherit
      (pkgs)
      # Browser automation for agents. `agent-browser` (vercel-labs) is the CLI
      # surface; `chromium` is the actual browser it drives.
      agent-browser
      chromium
      # Build-system layer above the base profile's cc + make: cmake /
      # ninja / pkg-config for the ecosystems that generate their builds;
      # rustup keeps the Rust toolchain pinnable per-project.
      cmake
      ninja
      pkg-config
      rustup
      ;
  };
}
