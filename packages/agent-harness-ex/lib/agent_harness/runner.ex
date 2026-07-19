defmodule AgentHarness.Runner do
  @moduledoc """
  The model-call seam: the one behaviour a host implements to plug a real
  LLM client into the harness. The library never talks to any API itself.

  `c:run/2` executes one full working phase of an agent, in a supervised
  task owned by that agent: the agentic loop of sampling the model,
  executing tool calls, and repeating until the model produces a final
  response. The contract with the harness:

    * Call `AgentHarness.checkpoint/2` after every tool result and splice
      the returned messages into the transcript. That is what implements the
      card's delivery rule ("inserted following the recipient's next tool
      result"); the harness only queues.
    * Call `AgentHarness.wait_for_message/3` when the model invokes its
      Wait for Message tool; the call blocks until a message arrives.
    * Report token consumption through `AgentHarness.add_usage/3` and stop
      when it returns `{:error, :budget_exhausted}`.
    * Return `{:ok, final_text}` with the agent's final response; the
      harness routes it to the lead and idles the agent. `{:error, reason}`
      (or a crash) reaches the lead as an `:error` message instead.

  Instructions are the only task context a subagent ever receives: the
  harness hands the runner exactly what the lead wrote, never the lead's own
  task description.
  """

  alias AgentHarness.Context

  @callback run(instructions :: String.t(), ctx :: Context.t()) ::
              {:ok, String.t()} | {:error, term()}
end
