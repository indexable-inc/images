defmodule SymphonyElixir.Runtime.Trigger do
  @moduledoc """
  The one matcher every producer shares to resolve an inbound trigger event
  to the `.sym` workflows that declared interest in it.

  A producer (a webhook, the operator dashboard) builds a trigger event
  map carrying `kind:` plus the kind's selector fields (a Slack channel,
  a GitHub `repo`/`label`, a Linear `label`), then hands it to
  `Runtime.Ingress.start_by_trigger/2`. That ingress asks
  `WorkflowCatalog.for_trigger_kind/1` for the candidates of that kind and
  keeps the ones `matches?/2` accepts. Cron does not route through here:
  its tick evaluates one catalog entry at a time and starts exactly that
  entry (`Ingress.start_workflow/3`), because a schedule is not an
  identity; matching on it would fire every workflow sharing the schedule.

  Keeping this predicate in one place is the point of the cutover: every
  producer used to re-implement its own `Catalog.dags() |> Enum.filter`
  match against the declared trigger. Now the selector vocabulary lives
  here, the producer carries only its event-shaped extraction and dedup,
  and a new selector field is one clause here rather than a new filter in
  each producer.

  The declared trigger is the workflow's `on` clause as the DSL parser
  normalizes it (`parser.ex` lifts it onto the AST and the catalog copies
  it to `entry.trigger`). The event is the runtime trigger map the producer
  builds; it stays the value stamped onto `RunGraph.trigger`, so a node can
  read its scope from `<input>`.
  """

  @typedoc "A workflow's declared `on` trigger, normalized by the parser."
  @type declared :: map() | nil

  @typedoc "A producer's inbound trigger event."
  @type event :: map()

  @doc """
  Does `declared` (a workflow's `on` clause) select the inbound `event`?

  The kinds already agree (the ingress filters by `kind` first), so this
  only compares the kind's selector fields:

  - `:linear` matches when the declared `label` is present on the event.
    The event carries the inbound issue's labels under `:labels`; a single
    matched label is enough.
  - `:github_pr_label` matches when both `repo` and `label` equal the
    event's.
  - `:slack_huddle_completed` and `:slack_app_mention` match when the
    declared `channel` equals the event's `channel` (a producer that
    resolves a channel name to an id stamps both so either compares equal).
  - `:manual` always matches its kind; an operator-started run names the
    workflow directly and never reaches this matcher.

  An event missing a selector the declared trigger requires does not match,
  so a malformed event fires nothing rather than fanning out to every
  workflow of that kind.
  """
  @spec matches?(declared(), event()) :: boolean()
  def matches?(%{kind: :linear, label: label}, %{labels: labels}) when is_list(labels), do: label in labels

  @spec matches?(declared(), event()) :: boolean()
  def matches?(%{kind: :github_pr_label, repo: repo, label: label}, event), do: event[:repo] == repo and event[:label] == label

  @spec matches?(declared(), event()) :: boolean()
  def matches?(%{kind: :slack_huddle_completed, channel: channel}, event), do: channel_matches?(channel, event)

  @spec matches?(declared(), event()) :: boolean()
  def matches?(%{kind: :slack_app_mention, channel: channel}, event), do: channel_matches?(channel, event)

  @spec matches?(declared(), event()) :: boolean()
  def matches?(%{kind: :manual}, _event), do: true

  @spec matches?(declared(), event()) :: boolean()
  def matches?(_declared, _event), do: false

  @doc """
  Normalize a label to this matcher's vocabulary: trimmed and lowercased.

  Both sides of a label comparison route through here (the DSL parser for
  a workflow's declared `label`, the webhook controllers for an inbound
  event's labels), so `"Bug "` and `"bug"` select the same workflows. A
  non-string (a malformed event payload) normalizes to `""`, which
  matches no declared label.
  """
  @spec normalize_label(term()) :: String.t()
  def normalize_label(label) when is_binary(label), do: label |> String.trim() |> String.downcase()
  def normalize_label(_label), do: ""

  # A Slack producer resolves the declared channel name (`#general`) to an
  # id once and stamps both `channel` and `channel_id` on the event, so the
  # declared name compares equal to either. Comparing against both keeps
  # this matcher independent of whether the producer worked in names or ids.
  defp channel_matches?(channel, event) do
    channel == event[:channel] or channel == event[:channel_id]
  end
end
