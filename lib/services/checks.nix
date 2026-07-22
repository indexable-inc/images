# Self-test for the mutable-json merge program (lib/services/mutable-json.nix),
# imported by lib/per-system.nix into the per-system check catalog.
{
  pkgs,
  mkCheck,
  mergeProgram,
}: {
  # Pins the last-applied 3-way merge behind homeModules.mutable-json:
  # first-install, preserve an app-written key, enforce a key the app
  # changed, prune a key Nix stopped declaring, and keep a sibling key
  # while a declared array is replaced atomically.
  mutable-json-merge = mkCheck "mutable-json-merge" {
    nativeBuildInputs = [pkgs.jq];
    script = ''
      prog=${mergeProgram}
      run() { jq -ncS --argjson last "$1" --argjson live "$2" --argjson new "$3" -f "$prog"; }
      check() {
        expected=$(printf '%s' "$2" | jq -cS .)
        if [ "$expected" != "$3" ]; then
          echo "FAIL $1: expected $expected got $3" >&2
          exit 1
        fi
        echo "ok $1"
      }
      check first-install '{"permissions":{"defaultMode":"bypass"}}' \
        "$(run '{}' '{}' '{"permissions":{"defaultMode":"bypass"}}')"
      check preserve-app-key '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' \
        "$(run '{"permissions":{"defaultMode":"bypass"}}' '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' '{"permissions":{"defaultMode":"bypass"}}')"
      check enforce-changed '{"permissions":{"defaultMode":"bypass"},"theme":"dark"}' \
        "$(run '{"permissions":{"defaultMode":"bypass"}}' '{"permissions":{"defaultMode":"off"},"theme":"dark"}' '{"permissions":{"defaultMode":"bypass"}}')"
      check prune-dropped '{"a":1,"c":3}' \
        "$(run '{"a":1,"b":2}' '{"a":1,"b":2,"c":3}' '{"a":1}')"
      check nested-atomic-array '{"p":{"allow":["x"]},"t":1}' \
        "$(run '{"p":{"allow":["x"]}}' '{"p":{"allow":["x","y"]},"t":1}' '{"p":{"allow":["x"]}}')"
      # Divergent live shape at a path we stop declaring must not abort:
      # the app replaced object `permissions` with a scalar, Nix dropped it.
      check divergent-live-shape '{"permissions":"all"}' \
        "$(run '{"permissions":{"defaultMode":"x"}}' '{"permissions":"all"}' '{}')"
    '';
  };
}
