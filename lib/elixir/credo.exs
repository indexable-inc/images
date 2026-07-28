# Single source of truth for the repo's Elixir static-analysis (Credo) policy,
# consumed by every Elixir quality gate through `ix.buildElixirCheck`
# (lib/build/elixir-check.nix) so it never drifts across packages. The Elixir
# counterpart of lib/ruff-ann.nix.
#
# Philosophy (same as ruff-ann.nix): turn on every check that catches a real bug,
# security issue, or stale idiom, and leave OFF the pure-consistency checks that
# would only generate suppression noise. Concretely that means `strict: true`
# (the low-priority refactor/readability checks gate too, not just warnings) PLUS
# the high-signal checks Credo ships disabled-by-default, MINUS four checks that
# are noise or duplicate another gate (see `disabled:` with per-check reasons).
#
# Run via `mix credo --strict` against this file. The gate is the sandboxed
# `*-elixir` flake check; `mix credo` in a package's aliases reuses the same file
# for local runs.
%{
  configs: [
    %{
      name: "default",
      # ExSlop (elixir-vibe/ex_slop): Credo plugin whose checks target LLM
      # failure modes (blanket rescues, narrator docs, identity passthrough,
      # try/rescue around non-raising calls). This repo's Elixir is
      # agent-written, exactly the code those checks were built for. The
      # default plugin config enables the high-signal checks and leaves the
      # noisy style ones off (#3876).
      plugins: [{ExSlop, []}],
      files: %{
        included: ["lib/", "test/", "config/", "mix.exs"],
        excluded: [~r"/_build/", ~r"/deps/"]
      },
      strict: true,
      color: false,
      # The map form keeps Credo's full default check set at its (strict) defaults,
      # then layers `extra:` on top and removes `disabled:`. No `:low` priority
      # downgrades: a flagged refactor/readability issue fails the build.
      checks: %{
        extra: [
          # --- security: real footguns (the Elixir analogue of ruff's bandit S) ---
          # Subprocess spawned without scrubbing the environment can leak secrets.
          {Credo.Check.Warning.UnsafeExec, []},
          # `Mix.env/0` referenced at runtime: it is a compile-time-only value.
          {Credo.Check.Warning.MixEnv, []},
          # `Map.get(map, :key)` where the key is required hides a nil bug; use
          # `map.key` / `Map.fetch!/2` so a missing key fails loudly.
          {Credo.Check.Warning.MapGetUnsafePass, []},

          # --- performance / modernization (the analogue of ruff's PERF + C4) ---
          # Each collapses a two-pass Enum chain into one pass, or a clearer call.
          {Credo.Check.Refactor.FilterReject, []},
          {Credo.Check.Refactor.RejectFilter, []},
          {Credo.Check.Refactor.FilterFilter, []},
          {Credo.Check.Refactor.MapMap, []},
          {Credo.Check.Refactor.MapJoin, []},
          {Credo.Check.Refactor.NegatedIsNil, []},
          {Credo.Check.Refactor.DoubleBooleanNegation, []},
          # `IO.puts`/`IO.inspect` for debugging left in source (lib/ IO.inspect is
          # also caught by astlog-rules/elixir.astlog; this covers IO.puts + test/).
          {Credo.Check.Refactor.IoPuts, []},

          # --- structure / consistency that genuinely aids readability ---
          # Enforce the canonical module layout (moduledoc, behaviour, use, import,
          # alias, require, module attrs, then defs).
          {Credo.Check.Readability.StrictModuleLayout, []},
          # Pick one of the multi vs single alias/import/require/use styles.
          {Credo.Check.Consistency.MultiAliasImportRequireUse, []},
          # Depth 3 (not Credo's pedantic default of 2): genuinely deep nesting
          # hurts, but a single guard inside a case inside a function is fine.
          {Credo.Check.Refactor.Nesting, max_nesting: 3}
        ],
        disabled: [
          # Both `_` (catch-all clauses) and `_name` (documenting an ignored
          # callback arg) are idiomatic Elixir and the codebase uses both on
          # purpose; forcing either way is churn that makes the code worse, not a
          # bug. Excluded for the same reason ruff-ann.nix drops pure-style rules.
          {Credo.Check.Consistency.UnusedVariableNames, []},
          # @spec enforcement lives in astlog-rules/elixir.astlog
          # (`public-def-needs-spec`, scoped to lib/). One concept, one gate:
          # keeping Credo's Specs too would double-report and demand @spec on test
          # fakes and mix.exs callbacks, which is noise.
          {Credo.Check.Readability.Specs, []},
          # @moduledoc enforcement also lives in astlog-rules/elixir.astlog
          # (`moduledoc-required`). Same one-concept-one-gate reason.
          {Credo.Check.Readability.ModuleDoc, []},
          # Subprocess calls here deliberately inherit the environment (git / ix
          # need PATH and friends). Exactly parallels ruff-ann.nix ignoring
          # S603/S607; the real "don't leak secrets" review is manual.
          {Credo.Check.Warning.LeakyEnvironment, []},
          # `list ++ [item]` outside a hot loop is clear and fine; the
          # order-preserving idiomatic rewrite (accumulate reversed, then
          # Enum.reverse/1) is a larger, bug-prone change for negligible gain.
          # Dropped for the same reason ruff-ann.nix drops PERF203.
          {Credo.Check.Refactor.AppendSingleItem, []}
        ]
      }
    }
  ]
}
