# Refreshes packages/mcp/pins.json from PyPI release metadata via the shared
# pin-update engine's `pypi` mode (packages/nix/pin-update); this file only
# renders the spec. Policy markers in pins.json the engine honors:
# - `prefetch = "manual"`: URL/hash stay hand-owned (non-sdist artifacts).
# - `hold = "<reason>"`: version, URL, and hash stay put.
# - `track = "<dotted prefix>"`: update only within that version line.
# Run from the repo root: `nix run .#mcp.updateScript`.
{pinUpdate}:
pinUpdate.mkUpdateScript {
  name = "mcp-pypi-pins-update";
  description = "Refresh packages/mcp/pins.json from PyPI release metadata";
  spec = {
    mode = "pypi";
    pins = "packages/mcp/pins.json";
  };
}
