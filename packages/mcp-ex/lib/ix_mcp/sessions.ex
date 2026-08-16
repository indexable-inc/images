defmodule IxMcp.Sessions do
  @moduledoc """
  The session directory and cross-session message bus (#3881). Kernel
  instances on a host already share one actions.db (#3880 made it the claim
  arbiter); this makes the agents riding them visible and addressable.
  `list/1` is the directory: each instance's sessions row joined with its
  current topic and a liveness heartbeat, so a cell can see who else is
  working before delegating or duplicating something another session
  already took. `send/3` addresses one session (by id, or by name when the
  name is unambiguous) and `broadcast/2` all of them: rows in the shared
  `session_messages` table, which every instance's `IxMcp.SessionWatch`
  sweeps and delivers into its agent's context as `source="sessions"`
  channel notifications within a few seconds.

  Same scope limit as the claim arbiter: the shared database is the bus, so
  the directory and messaging are per host. Cross-host messaging would ride
  the fleet, out of scope here (#3881).
  """

  alias IxMcp.ActionLog
  alias IxMcp.Session

  # A live instance heartbeats every SessionWatch tick (seconds) and on
  # transport register; half a minute of silence means gone, not slow.
  # Generous on purpose: cells run in their own processes, so even a kernel
  # full of wedged jobs keeps ticking -- only a dead one stops.
  @fresh_within_s 30

  @typedoc "A directory row: the recorded session plus the computed flags."
  @type entry :: %{
          id: integer(),
          name: String.t() | nil,
          topic: String.t() | nil,
          started_at: String.t(),
          last_seen_at: String.t() | nil,
          parent: integer() | nil,
          spawn_tag: String.t() | nil,
          live: boolean(),
          self: boolean()
        }

  @doc """
  The session directory: rows with a heartbeat plus this session, freshest
  first, each flagged `live:` (heartbeat within #{@fresh_within_s}s) and
  `self:`. Rows that never heartbeat are dead history (pre-#3881 instances,
  one-shot connections) and stay hidden; `all: true` includes them.

  Subagent rows (`parent` set, ENG-12004) are hidden by default: this list
  is who a cell can delegate to, and a registered child has no kernel to
  read a delegation with until ENG-12004 phase 3. `children: true` includes
  them.

  Options exist for tests: `:action_log` (the shared log), `:session_id`
  (this session's row), `:now` (the liveness clock).
  """
  @spec list(keyword()) :: [entry()]
  def list(opts \\ []) do
    log = Keyword.get(opts, :action_log, ActionLog)
    self_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)
    now = Keyword.get_lazy(opts, :now, &DateTime.utc_now/0)

    ActionLog.session_directory(log)
    |> Enum.filter(fn row ->
      (Keyword.get(opts, :children, false) or row.parent == nil) and
        (Keyword.get(opts, :all, false) or row.last_seen_at != nil or row.id == self_id)
    end)
    |> Enum.map(fn row ->
      row
      |> Map.put(:live, live?(row, now))
      |> Map.put(:self, row.id == self_id)
    end)
    |> Enum.sort_by(fn row -> row.last_seen_at || "" end, :desc)
  end

  @doc """
  Message another session: an integer sends to that directory id, a string
  to the unique session with that name (ambiguity is an error naming the
  candidates -- use the id). Returns `{:ok, detail}`; a stale target is
  still sent to, but the detail says its heartbeat stopped, because a dead
  instance never reads its mail.

  Same test options as `list/1`.
  """
  @spec send(integer() | String.t(), String.t(), keyword()) ::
          {:ok, String.t()} | {:error, String.t()}
  def send(target, text, opts \\ []) when is_binary(text) do
    log = Keyword.get(opts, :action_log, ActionLog)
    self_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)
    now = Keyword.get_lazy(opts, :now, &DateTime.utc_now/0)

    with {:ok, row} <- resolve(target, self_id, log, now) do
      case ActionLog.send_session_message(self_id, row.id, text, log) do
        {:ok, message} ->
          {:ok, "sent to session #{label(row)} at #{message.created_at}" <> stale_note(row, now)}

        :disabled ->
          {:error, "action log disabled (#3539); no shared database to carry the message"}
      end
    end
  end

  @doc """
  Message every session on the host (a NULL `to_session` row): each live
  instance's sweep delivers it. Same test options as `list/1`.
  """
  @spec broadcast(String.t(), keyword()) :: {:ok, String.t()} | {:error, String.t()}
  def broadcast(text, opts \\ []) when is_binary(text) do
    log = Keyword.get(opts, :action_log, ActionLog)
    self_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)

    case ActionLog.send_session_message(self_id, nil, text, log) do
      {:ok, message} ->
        peers =
          opts
          |> Keyword.merge(action_log: log, session_id: self_id)
          |> list()
          |> Enum.count(fn row -> row.live and not row.self end)

        {:ok, "broadcast at #{message.created_at}; #{peers} live peer(s) will hear it"}

      :disabled ->
        {:error, "action log disabled (#3539); no shared database to carry the message"}
    end
  end

  defp resolve(id, self_id, log, _now) when is_integer(id) do
    cond do
      id == self_id ->
        {:error, "session #{id} is this session; message a peer from Sessions.list()"}

      row = Enum.find(ActionLog.session_directory(log), fn row -> row.id == id end) ->
        refuse_child(row)

      true ->
        {:error, "no session #{id} in the directory; Sessions.list() shows who is here"}
    end
  end

  # A name resolves against live sessions first: names recur across an
  # instance's many lifetimes, but only one incarnation heartbeats. With no
  # live match a single stale one is still unambiguous; several matches in
  # the same liveness tier need the id.
  defp resolve(name, self_id, log, now) when is_binary(name) do
    matches =
      ActionLog.session_directory(log)
      |> Enum.filter(fn row -> row.name == name and row.id != self_id and row.parent == nil end)

    case {Enum.filter(matches, &live?(&1, now)), matches} do
      {[row], _} -> {:ok, row}
      {[], []} -> {:error, "no session named #{inspect(name)}; Sessions.list() shows who is here"}
      {[], [row]} -> {:ok, row}
      {[], stale} -> {:error, ambiguous(name, stale)}
      {live, _} -> {:error, ambiguous(name, live)}
    end
  end

  # A registered subagent has no kernel sweeping its mailbox until
  # ENG-12004 phase 3, so a send would sit unread while looking delivered.
  # Its lead's `Agents.send/2` is the channel that actually reaches it.
  defp refuse_child(%{parent: parent} = row) when is_integer(parent) do
    {:error,
     "session #{label(row)} is a subagent of session #{parent} and has no kernel to read " <>
       "mail with (ENG-12004); its lead reaches it via Agents.send/2"}
  end

  defp refuse_child(row), do: {:ok, row}

  defp ambiguous(name, rows) do
    ids = Enum.map_join(rows, ", ", fn row -> "#{row.id} (last seen #{row.last_seen_at})" end)
    "#{inspect(name)} names sessions #{ids}; send to the id instead"
  end

  defp label(%{name: nil, id: id}), do: "##{id}"
  defp label(%{name: name, id: id}), do: "#{name} (##{id})"

  defp stale_note(row, now) do
    if live?(row, now) do
      ""
    else
      "; its heartbeat stopped (last seen #{row.last_seen_at || "never"}), it may never read this"
    end
  end

  defp live?(%{last_seen_at: nil}, _now), do: false

  defp live?(%{last_seen_at: at}, now) do
    case DateTime.from_iso8601(at) do
      {:ok, seen, _offset} -> DateTime.diff(now, seen) <= @fresh_within_s
      _ -> false
    end
  end
end
