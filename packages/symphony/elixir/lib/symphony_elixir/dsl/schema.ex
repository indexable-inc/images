defmodule SymphonyElixir.DSL.Schema do
  @moduledoc """
  One JSON-able snapshot of the runtime's vocabulary: the engines, efforts,
  permission levels, placement locations, node kinds, node states, effect
  kinds, and trigger kinds the runtime actually accepts.

  Each field reads the single source-of-truth accessor on the module that
  owns the enum, so the schema cannot drift from what a turn, a node, or
  the parser will take. Forms and renderers build their option lists from
  `to_map/0` rather than hard-coding literals, so adding an enum value at
  its owner flows to the UI without a form edit.
  """

  alias SymphonyElixir.DSL.{AST, Parser}
  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Node

  @doc """
  Collect the runtime enums into one map keyed by domain. Values are atoms
  straight from each accessor; `Jason.encode/1` renders them as strings, so
  a consumer reads `"codex"`, `"high"`, `"workspace_write"`, and so on.
  """
  @spec to_map() :: %{atom() => [atom()]}
  def to_map do
    %{
      engines: Envelope.engines(),
      efforts: Envelope.efforts(),
      permissions: Envelope.permission_levels(),
      locations: Envelope.locations(),
      node_kinds: Node.kinds(),
      node_states: Node.states(),
      effect_kinds: AST.effect_kinds(),
      trigger_kinds: Parser.trigger_kinds()
    }
  end
end
