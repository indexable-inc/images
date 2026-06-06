defmodule SymphonyElixir.DSL.SchemaTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.DSL.{AST, Parser, Schema}
  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Node

  describe "to_map/0" do
    test "each field is the owner's accessor verbatim" do
      # The point of the schema is that it does not restate the enums: it
      # reads each owner's single source of truth. Asserting equality here
      # means adding a value at the owner flows through with no schema edit
      # and no UI edit.
      schema = Schema.to_map()

      assert schema.engines == Envelope.engines()
      assert schema.efforts == Envelope.efforts()
      assert schema.permissions == Envelope.permission_levels()
      assert schema.locations == Envelope.locations()
      assert schema.node_kinds == Node.kinds()
      assert schema.node_states == Node.states()
      assert schema.effect_kinds == AST.effect_kinds()
      assert schema.trigger_kinds == Parser.trigger_kinds()
    end

    test "every value is a list of atoms, so it encodes to JSON as strings" do
      schema = Schema.to_map()

      for {_key, values} <- schema do
        assert is_list(values)
        assert Enum.all?(values, &is_atom/1)
      end

      assert {:ok, _json} = Jason.encode(schema)
    end
  end

  describe "trigger_kinds/0" do
    test "every advertised trigger kind parses through an `on` clause" do
      # Guard against the accessor drifting from the parser's dispatch: a
      # kind the schema offers but the parser rejects would be a dead UI
      # option. Each kind gets its minimal valid params.
      params = %{
        manual: "",
        cron: ~s|"0 * * * *"|,
        linear: ~s|label "ready"|,
        slack_huddle: ~s|channel "C123"|,
        slack_mention: ~s|channel "C123"|,
        github_pr_label: ~s|repo "owner/name" label "ship"|
      }

      for kind <- Parser.trigger_kinds() do
        clause = Map.fetch!(params, kind)
        source = ~s|workflow "w" on #{kind} #{clause} { a <- exec "./x" }|

        assert {:ok, ast} = Parser.parse(source), "expected #{kind} to parse"
        assert is_map(ast.trigger)
      end
    end
  end
end
