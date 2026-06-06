%{
  configs: [
    %{
      name: "default",
      files: %{
        included: ["lib/", "test/", "config/", "mix.exs"],
        excluded: []
      },
      strict: false,
      color: true,
      checks: [
        # Make refactoring suggestions informational rather than CI-failing.
        # CI catches real correctness issues via mix compile --warnings-as-errors;
        # credo's refactor suggestions are useful local feedback but should not
        # gate the build on every threshold tweak.
        {Credo.Check.Refactor.CyclomaticComplexity, max_complexity: 12, priority: :low},
        {Credo.Check.Refactor.Nesting, max_nesting: 3, priority: :low},
        {Credo.Check.Refactor.WithClauses, priority: :low},
        {Credo.Check.Refactor.RedundantWithClauseResult, priority: :low},
        {Credo.Check.Refactor.CondStatements, priority: :low},
        {Credo.Check.Readability.WithSingleClause, priority: :low},
        # Config is intentionally a wide snapshot of env vars. Splitting into
        # nested substructs would just push the same field count into nested
        # types without making boot-time wiring clearer, and would break the
        # field-name == opt-key round-trip the snapshot relies on.
        {Credo.Check.Warning.StructFieldAmount, max_fields: 60}
      ]
    }
  ]
}
