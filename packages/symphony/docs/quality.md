# Quality gate

Symphony runs a quality gate that reports formatting, lint, static security,
dependency-audit, type, and coverage findings. It is informational today and
does not block PRs. Run it locally with one command:

```sh
cd elixir
make quality
```

That target runs `mix quality` (format check, Credo strict, Sobelow, deps
audit, Dialyzer) followed by `mix coveralls`. None of these are part of `make
all`; the gate lives in its own `quality` target.

## Tools

- `mix format --check-formatted`: fails if any file is not formatted to the
  rules in `elixir/.formatter.exs` (200-column lines).
- `mix credo --strict`: lint and refactoring analysis. Strict mode surfaces all
  priorities, including the low-priority refactor checks pinned in
  `elixir/.credo.exs`.
- `mix sobelow --config`: static security scanner for Phoenix apps, reading
  `elixir/.sobelow-conf`. Reports common web vulnerabilities (XSS, CSRF,
  config, traversal). Reporting only: it does not set an `exit` threshold.
- `mix deps.audit`: checks the dependency tree in `mix.lock` against the
  Elixir security advisory database (`mix_audit`).
- `mix dialyzer`: success-typing analysis (`dialyxir`). The PLT is built under
  `elixir/priv/plts/` (gitignored) and cached in CI keyed on the toolchain and
  `mix.lock`.
- `mix coveralls`: test-suite line coverage total (`excoveralls`).

## CI

The standalone symphony repo ran `make all` and `make quality` through its own
GitHub workflows. Those did not survive the move into the index monorepo:
GitHub only reads workflows from the repository root, so the nested copies were
deleted rather than left inert. Index CI builds `room-server` through Nix
(`pkgs.symphony-room-server`) but does not run the Elixir suite or the quality
gate; run both locally until an index CI lane picks them up.

## Phased rollout

The gate ships in two phases so it never blocks PRs while the codebase is still
being brought into compliance.

### Phase A (this PR, WS-8): tooling plus non-blocking reporting

Install the tools, add the `quality` Make target and alias, and surface a
violations summary. Nothing here makes `make all` stricter. The point is to see
the violations, not to enforce them yet.

### Phase B (WS-9, after the overhaul cutover): enforce

Phase B lands only after the top-down overhaul cutover, once the module set is
final, so we do not spend effort on modules the cutover deletes. Steps:

1. One-time Styler reformat, then enable the Styler formatter plugin in
   `.formatter.exs`.
2. Add Boundary as a dep and `use Boundary` annotations encoding the layer
   rules: DSL -> IR -> Runtime -> `Engine.Client`; `Engine.Client` is the only
   door to the room-server; `bridge`/`state`/`http` never name a concrete
   engine.
3. Fix the `credo --strict` and Dialyzer violations.
4. Wire `make quality` into index CI as a required check.

Boundary is deferred until post-cutover on purpose. The module topology is
still changing in the overhaul, so annotating modules now would encode layer
rules onto modules the cutover removes. Boundary annotations land in Phase B
against the final module set.
