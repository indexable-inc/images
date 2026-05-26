## Nix style (ast-grep enforced)

Run `nix run .#lint` before committing. It runs nixfmt, Statix, Deadnix, and the
repo's ast-grep rules. The lint app is the mechanical source of truth. The
common hard rules are:

- No `with pkgs;` or `with lib;`. Use `inherit (pkgs) ...` or `lib.foo`
  directly.
- No `rec { }`. Use `let ... in` or `final` / `prev`.
- No `mkForce`. Fix the module boundary or compose priorities deliberately.
- No `lib.recursiveUpdate`. Build the attrset in one place or use `lib.mkMerge`.
- No repeated parent keys in the same attrset. Group related assignments under
  one parent.
- Prefer `inherit (source) name;` for direct same-name field copies.
- No `builtins.currentSystem`, `builtins.getEnv`, `<nixpkgs>`, or `path:` flake
  refs.
- No `(import ./foo.nix)` inside `imports = [ ... ]`; NixOS auto-imports paths.
- No `..` paths inside `modules/`; shared helpers come through `specialArgs.ix`.
- No `writeShellApplication` or `writeShellScriptBin` for user-facing commands.
- No bare `assert cond;`. Use an assertion that names the failure.
- No unused bindings. Use `_` for intentionally unused lambda arguments.
- Set `strictDeps = true` on every `mkDerivation`.
- Keep raw fetched data artifact URLs out of `flake.nix`.
- Use `pkgs.*` fetchers instead of `builtins.fetch*`.
- Commit real hashes, never fake hash helpers or placeholders.
- Use `nixosModules.<name>` for module exports. Avoid a flat top-level
  `modules` output.
- Keep image targets at `x86_64-linux`.
- Use structured config options for new modules instead of stringly config
  fragments.

