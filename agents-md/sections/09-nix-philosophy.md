## Nix philosophy

`flake.nix` is the manifest. It should expose inputs and delegate outputs to
`lib/`, discovery, and package-specific files. Keep scenario wiring, artifact
manifests, app wrappers, and helper logic near the owner that changes with them.

Use standard flake outputs: `packages`, `checks`, `formatter`, `devShells`,
`templates`, `overlays`, `nixosModules`, and `lib`. A workflow command should be
a package with `meta.mainProgram` so `nix run .#<name>` and
`nix build .#<name>` point at the same derivation.

Composition belongs in this repo; low-level ix VM primitives belong in `ix`.
Build workflows by consuming stable primitives, rendered plans, and plain data
surfaces. Add CLI primitives only when the lower layer truly owns the behavior.

Expose aggregate knowledge as data before wrapping it in a command. A
`lib.<name>` value that `nix eval --json` can inspect is easier to reuse than a
one-off app. Add a wrapper when formatting, joins, follow-up actions, or
human-facing output justify it.

Prefer one source of truth. Discovery beats hand-maintained registries. Generated
catalogs should come from small manifests. Hashes live with URLs. Versions live
near the image, package, or ecosystem that owns them.

Keep eval pure. Inputs flow through `flake.nix` or typed parameters. Avoid host
environment reads, channel refs, ad hoc flake paths, and eval-time network
fetches.

Import From Derivation is acceptable when another tool must reveal the real
build graph. Keep the boundary explicit, expose the generated artifact, and
batch discovery into one larger derivation when many tiny IFDs would serialize
the evaluator.

Generate commands through checked helpers. A wrapper reached through
`nix run .#...` should call realized executables with `lib.getExe`, an app
program, or an explicit store path reference. Avoid nesting another flake
frontend inside the generated command.

Nix builders for language workspaces should pass the smallest source closure the
compiler can consume. The caller names both the filtered `src` and the real
`workspaceRoot`; do not infer one from the other.

