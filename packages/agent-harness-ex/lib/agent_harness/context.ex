defmodule AgentHarness.Context do
  @moduledoc """
  Per-agent handle passed to a `AgentHarness.Runner` alongside its instructions.

  It carries everything the runner needs to talk back to the harness
  (`harness` + `agent_id` address the agent's own mailbox through the public
  `AgentHarness` functions) plus the per-spawn knobs the harness itself never
  interprets: `model` is an opaque term chosen at `create_subagent` time, and
  `opts` is the full spawn option list for host-specific extras.
  """

  @enforce_keys [:harness, :agent_id, :model, :token_budget]
  defstruct [:harness, :agent_id, :model, :token_budget, opts: []]

  @type t :: %__MODULE__{
          harness: atom(),
          agent_id: String.t(),
          model: term(),
          token_budget: pos_integer(),
          opts: keyword()
        }
end
