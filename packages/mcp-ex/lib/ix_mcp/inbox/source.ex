defmodule IxMcp.Inbox.Source do
  @moduledoc """
  What an inbound-message feed must know how to do, so that
  `IxMcp.Inbox.Watcher` can own the polling loop once.

  The loop is where the subtle bugs live -- a watermark that advances past
  messages nobody heard, a retry storm against a service that is merely
  asleep -- so it is written once and both feeds share it. A source is then
  small enough to read in a sitting: decide whether it can run at all,
  answer "what arrived after this instant", and normalize each hit into
  whatever its `c:renderer/0` prints.

  ## The fetch contract

  `c:fetch/3` gets a lower bound and a limit and returns items OLDEST
  FIRST. Both services answer newest-first, so each source reverses: the
  announcement order is then the order the messages were sent, which is the
  order a reader can follow.

  The third element of the reply says whether the window held MORE than the
  limit. It is not a detail. Both services cap a page at the NEWEST N, so
  the messages a cap drops are the oldest unseen ones, and a source that
  reported nothing about the cap would make a busy minute look like a quiet
  one.
  """

  @typedoc "Whatever the source needs to answer a fetch: endpoint, token, client module."
  @type state :: term()

  @typedoc """
  One item a sweep found.

  The loop reads only `:id`, which is the dedup key across the overlap
  re-read. Every other field is a contract between the source and its
  `c:renderer/0` and nothing in between looks at it -- which is what lets a
  feed of CI verdicts share the loop with a feed of chat messages instead of
  pretending a build has a sender.
  """
  @type item :: %{required(:id) => String.t(), optional(atom()) => term()}

  @doc """
  Decide whether this feed can run, and build its state.

  `:ignore` is the normal answer on a machine that has no such account: the
  watcher then never starts, and nothing is logged about it. Credentials are
  read here, once, rather than on every sweep.
  """
  @callback init(keyword()) :: {:ok, state()} | :ignore

  @doc """
  Short wire label, reaching the client as `meta.source`: "beeper" or "mail".
  """
  @callback label() :: String.t()

  @doc """
  Poll cadence in ms.

  Owned by the source because the costs differ by orders of magnitude: an
  HTTP call to a desktop app on loopback is nearly free, while a mail search
  that costs a metadata fetch per hit is not.
  """
  @callback default_interval_ms() :: pos_integer()

  @doc """
  Messages strictly after `since`, at most `limit` of them, oldest first.

  Returns `{:ok, items, more_than_limit?, state}`. An `{:error, detail}` is
  expected and survivable -- the desktop app is closed, a token went stale,
  the network blinked -- and the watcher backs off without advancing its
  watermark, so nothing is lost by failing.

  `detail` is for a log line, so it must carry no message content.
  """
  @callback fetch(state(), DateTime.t(), pos_integer()) ::
              {:ok, [item()], boolean(), state()} | {:error, String.t()}

  @doc """
  The `IxMcp.Inbox.Renderer` that turns this source's items into lines.

  Optional: a source that does not answer is a chat feed and gets
  `IxMcp.Inbox.Announce`.
  """
  @callback renderer() :: module()

  @doc """
  How far back the FIRST sweep looks, in seconds.

  Optional, and 0 for the chat feeds: a session opening with the last minutes
  of someone else's conversation is noise. It is not noise for a feed whose
  items are slower than a kernel restart -- a CI run outlives one, so a
  session that starts just after a rebuild would otherwise never hear the
  verdict of the run it was waiting on. Bounded above by the watcher's
  `:max_backfill_s` either way.
  """
  @callback initial_backfill_s() :: non_neg_integer()

  @optional_callbacks renderer: 0, initial_backfill_s: 0

  @doc """
  A positive-integer override from the environment, or `default`.

  Shared so that every source reads an override the same way, and so that a
  typo (`""`, `"fast"`, `"0"`) falls back to the documented default instead
  of turning into a busy loop.
  """
  @spec interval_from_env(String.t(), pos_integer()) :: pos_integer()
  def interval_from_env(name, default) when is_binary(name) and is_integer(default) do
    case Integer.parse(System.get_env(name, "")) do
      {value, ""} when value > 0 -> value
      _ -> default
    end
  end
end
