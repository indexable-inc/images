# Default ix environment base: agent CLIs plus a normal build toolchain.
# The auto-enabled base profile supplies version control, editors, the nushell
# workspace wrapper, debuggers, tracing tools, and archive utilities.
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
      # Build toolchain. Most ecosystems lean on cmake / make / ninja and
      # pkg-config; rustup keeps the toolchain pinnable per-project.
      cmake
      gcc
      gnumake
      ninja
      pkg-config
      rustup
      # Default language runtimes that show up across most dev sessions.
      nodejs
      python3
      ;
  };
}
