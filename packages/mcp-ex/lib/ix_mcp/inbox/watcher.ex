defmodule IxMcp.Inbox.Watcher do
  @moduledoc """
  One polling loop over an `IxMcp.Inbox.Source`, announcing every new
  inbound message on the channel the way job finishes are.

  The ask this answers: a message that arrives while a session is running
  should reach the session -- on every bridged platform and in mail --
  without anyone having asked for it first. So both feeds are ON by default
  and configured by nothing. What makes that safe to ship to a machine with
  neither account is `c:IxMcp.Inbox.Source.init/1` returning `:ignore` when
  it finds no credential: the watcher then never starts and says nothing.

  ## Why the watermark does not move while nobody is listening

  A channel event is not durable. `IxMcp.MCP.Notifier.publish/1` writes an
  outbox row and replays it when a transport rejoins, but
  `IxMcp.MCP.Notifier.channel/2` is a fan-out to whoever is attached right
  now -- with zero transports it is dropped, and nothing anywhere records
  that it existed. A sweep that announced into an empty room and then
  advanced its watermark would bury those messages permanently. This is the
  same trap the fleet alerts avoid by asking `transports/0` before writing
  their "already announced" fingerprint, and the answer is the same: skip
  the sweep entirely, leave the watermark where it is, and let the window
  widen until a session comes back.

  Two bounds keep the catch-up from becoming a flood. `:max_backfill_s`
  caps how far back a re-attached session looks, so a kernel left running
  overnight does not open with hours of chat, and `:limit` caps one sweep
  with the overflow said out loud instead of swallowed.

  ## Overlap, not a boundary

  Each fetch's lower bound is the previous sweep's start MINUS an overlap,
  because a bridged message can land carrying a timestamp a few seconds in
  the past -- the bridge saw it before the desktop app filed it -- and a
  strict boundary steps straight over those. Re-reading the overlap window
  is what makes `seen` load-bearing: it holds the ids the last fetch
  returned, so each message is announced exactly once. Same shape as
  `IxMcp.IssueWatch`, for the same reason.

  ## One loop, one line format per feed

  The loop is generic over BOTH halves of a feed: `IxMcp.Inbox.Source` says
  how to read, and `c:IxMcp.Inbox.Source.renderer/0` says how to print. A
  source that names no renderer gets `IxMcp.Inbox.Announce`, so the chat
  feeds are unchanged, while `IxMcp.Forge.Verdicts` prints CI verdicts
  through its own renderer without a second copy of everything above.

  Deliberately NOT here: replying, marking read, or any other write. This
  is a feed. It reads, and it tells you what it read.
  """

  use GenServer

  alias IxMcp.Inbox.Announce
  alias IxMcp.MCP.Notifier

  require Logger

  # One sweep's cap. Twenty lines is already a wall of text in a session;
  # beyond that the overflow line is more useful than the messages.
  @limit 20
  @overlap_s 60
  # Fifteen minutes: long enough to cover a lunch break or a client
  # reconnect, short enough that no catch-up is a scroll of history.
  @max_backfill_s 900
  @max_backoff_ms 60_000
  # 2^8 * any sane interval is already past the cap; the clamp only keeps
  # the shift itself from growing without bound on a long outage.
  @max_backoff_shift 8

  @doc """
  Start a feed.

  Options: `:source` (required, the module implementing
  `IxMcp.Inbox.Source`), `:interval_ms`, `:limit`, `:overlap_s`,
  `:max_backfill_s`, `:initial_backfill_s`, `:renderer`, `:transports` (a
  0-arity fun, for tests), `:name` (defaults to the source module, `nil` for
  an unregistered process). Every other option is passed through to the
  source's `init/1`.
  """
  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts) do
    source = Keyword.fetch!(opts, :source)
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, source))
  end

  @doc """
  Child spec keyed by source, so one supervisor can run both feeds: the
  default spec ids by module, and both children are this module.
  """
  @spec child_spec(keyword()) :: Supervisor.child_spec()
  def child_spec(opts) do
    %{
      id: {__MODULE__, Keyword.fetch!(opts, :source)},
      start: {__MODULE__, :start_link, [opts]}
    }
  end

  @impl true
  def init(opts) do
    source = Keyword.fetch!(opts, :source)

    case source.init(opts) do
      :ignore ->
        :ignore

      {:ok, source_state} ->
        {:ok, schedule(state(source, source_state, opts))}
    end
  end

  defp state(source, source_state, opts) do
    %{
      source: source,
      source_state: source_state,
      renderer: Keyword.get(opts, :renderer, renderer(source)),
      interval_ms: Keyword.get(opts, :interval_ms, source.default_interval_ms()),
      limit: Keyword.get(opts, :limit, @limit),
      overlap_s: Keyword.get(opts, :overlap_s, @overlap_s),
      max_backfill_s: Keyword.get(opts, :max_backfill_s, @max_backfill_s),
      transports: Keyword.get(opts, :transports, &Notifier.transports/0),
      since: DateTime.add(DateTime.utc_now(:second), -backfill(source, opts), :second),
      seen: MapSet.new(),
      failures: 0
    }
  end

  # Both optional callbacks are resolved once, at start, and only after
  # `source.init/1` has run -- which is what guarantees the module is loaded
  # and `function_exported?/3` can answer.
  defp renderer(source) do
    if function_exported?(source, :renderer, 0), do: source.renderer(), else: Announce
  end

  defp backfill(source, opts) do
    default =
      if function_exported?(source, :initial_backfill_s, 0),
        do: source.initial_backfill_s(),
        else: 0

    Keyword.get(opts, :initial_backfill_s, default)
  end

  @impl true
  def handle_info(:poll, state) do
    {:noreply, state |> sweep() |> schedule()}
  end

  defp schedule(state) do
    Process.send_after(self(), :poll, backoff(state))
    state
  end

  # A closed desktop app or a stale token must not become a poll storm, and
  # must not become a log storm either: the interval doubles per consecutive
  # failure up to a minute, and resets on the first success.
  defp backoff(%{failures: 0} = state), do: state.interval_ms

  defp backoff(state) do
    shift = min(state.failures, @max_backoff_shift)
    min(state.interval_ms * Integer.pow(2, shift), @max_backoff_ms)
  end

  defp sweep(state) do
    if state.transports.() == 0 do
      state
    else
      collect(state, DateTime.utc_now(:second))
    end
  end

  defp collect(state, now) do
    case state.source.fetch(state.source_state, lower_bound(state, now), state.limit) do
      {:ok, items, more?, source_state} ->
        announce(state, items, more?)

        %{
          state
          | since: now,
            seen: MapSet.new(items, & &1.id),
            source_state: source_state,
            failures: 0
        }

      {:error, detail} ->
        # The watermark stays put, so a failed sweep loses nothing: the next
        # success covers the same window. Only the failure is logged, never
        # a message, and `detail` is contracted to carry no content.
        Logger.warning("#{state.source.label()} feed sweep failed: #{detail}")
        %{state | failures: state.failures + 1}
    end
  end

  defp announce(state, items, more?) do
    label = state.source.label()
    fresh = Enum.reject(items, &MapSet.member?(state.seen, &1.id))

    Enum.each(fresh, &state.renderer.announce(label, &1))

    # Only when something was actually new: `more?` stays true on every
    # sweep of a busy window, and an overflow line per idle sweep would be
    # the noise this feed is trying to be quiet about.
    if more? and fresh != [] do
      state.renderer.announce_overflow(label, state.limit)
    end

    :ok
  end

  # The previous sweep's start, widened by the overlap, but never further
  # back than the backfill cap.
  defp lower_bound(state, now) do
    overlapped = DateTime.add(state.since, -state.overlap_s, :second)
    oldest = DateTime.add(now, -state.max_backfill_s, :second)

    if DateTime.compare(overlapped, oldest) == :lt, do: oldest, else: overlapped
  end
end
