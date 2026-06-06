defmodule SymphonyElixir do
  @moduledoc """
  Symphony runs workflows of agent invocations.

  - A `workflows/<name>.sym` file declares the nodes and edges of a workflow
    in the `.sym` surface language, lowered to an IR run graph.
  - A `skills/<name>.md` file declares the system prompt, codex policy, and
    tool surface a `skill "name"` prompt resolves to.
  - A trigger (Linear label, manual API call, cron tick, Slack, GitHub)
    starts a run.
  - Each run gets a fresh workspace from the primary repository's configured
    default branch.
  - The IR runtime walks the graph, executing one node at a time through the
    engine host.
  """
end
