defmodule IxMcp.Requests do
  @moduledoc """
  The per-host work bus (#3883), generalizing issue pickup (#3880): any unit
  of work -- "review this diff", "run this eval", "take over this PR" -- can
  be posted from any cell, and any agent on the host can claim it
  atomically. `post/3` offers work (a `requests` row in the shared
  actions.db, announced to every session as a `source="requests"` channel
  event); `pickup/2` claims one before working it, a single guarded UPDATE
  in the single-writer `IxMcp.ActionLog` whose row count decides the race;
  `done/2` finishes it; `list/1` shows the board, open first.
  `IxMcp.Issues.pickup/1` is a veneer over this bus: an issue claim is a
  request of kind `:issue` whose ref is the `"owner/repo#n"`.

  Same scope limit as the rest of the shared database: the bus is per host.
  Cross-host requests would ride the fleet, out of scope here (#3883).
  """

  alias IxMcp.ActionLog
  alias IxMcp.Session

  @doc """
  Offer work to every agent on the host: inserts an open request and
  announces it (every instance's `IxMcp.SessionWatch` delivers a
  `source="requests"` `event="posted"` channel notification within
  seconds). `kind: :issue` with `ref: "owner/repo#n"` posts a GitHub-backed
  request instead of the default `:adhoc`; posting a ref that already has a
  row reads the standing row back (idempotent ensure) and says so.

  Options exist for tests: `:action_log` (the shared log), `:session_id`
  (the posting sessions row).
  """
  @spec post(String.t(), String.t() | nil, keyword()) :: {:ok, String.t()} | {:error, String.t()}
  def post(title, body \\ nil, opts \\ []) when is_binary(title) do
    log = Keyword.get(opts, :action_log, ActionLog)
    session_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)

    with {:ok, kind, ref} <- kind_and_ref(opts) do
      case ActionLog.post_request(kind, ref, title, body, session_id, log) do
        # An adhoc post always inserts; an issue-kind post is an ensure, so
        # the row may predate this call -- report its standing state rather
        # than claim a fresh offer.
        {:ok, %{kind: :adhoc} = request} ->
          {:ok,
           "posted request ##{request.id}: #{request.title}; " <>
             "every session on the host hears it"}

        {:ok, request} ->
          {:ok, "request ##{request.id} for #{ref}: #{describe(request)}"}

        :disabled ->
          {:error, "action log disabled (#3539); no shared database to post on"}
      end
    end
  end

  @doc """
  Claim request `id` atomically BEFORE working it. `{:ok, detail}` means
  this session won -- do the work, then `done/2` it. A lost claim returns
  `{:error, "claimed by session <label> ..."}`: pick different work.
  Re-picking a request this session already holds is a win, not a loss
  (#3903). Same test options as `post/3`.
  """
  @spec pickup(integer(), keyword()) :: {:ok, String.t()} | {:error, String.t()}
  def pickup(id, opts \\ []) when is_integer(id) do
    log = Keyword.get(opts, :action_log, ActionLog)
    session_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)

    case ActionLog.claim_request(id, session_id, log) do
      {:ok, request} ->
        {:ok, "claimed request ##{request.id} (#{request.title}) at #{request.claimed_at}"}

      {:error, :not_found} ->
        {:error, "no request ##{id}; Requests.list() shows the board"}

      {:error, request} ->
        {:error, describe(request)}

      :disabled ->
        # No arbiter, no claim: pretending to win here is exactly the double
        # pickup this module exists to prevent.
        {:error, "action log disabled (#3539); no arbiter to claim through"}
    end
  end

  @doc """
  Mark claimed request `id` done, announcing `event="done"` to the host.
  Finishing an already-done request is idempotent; a still-open one is an
  error -- claim what you work. Same test options as `post/3`.
  """
  @spec done(integer(), keyword()) :: {:ok, String.t()} | {:error, String.t()}
  def done(id, opts \\ []) when is_integer(id) do
    log = Keyword.get(opts, :action_log, ActionLog)
    session_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)

    case ActionLog.finish_request(id, session_id, log) do
      {:ok, request} ->
        {:ok, "request ##{request.id} (#{request.title}) done at #{request.done_at}"}

      {:error, :not_found} ->
        {:error, "no request ##{id}; Requests.list() shows the board"}

      {:error, request} ->
        {:error, "request ##{request.id} is still open; pickup before finishing it"}

      :disabled ->
        {:error, "action log disabled (#3539); no shared database to record it"}
    end
  end

  @doc """
  The request board: every request as a map, open first (then claimed, then
  done), newest first within a status. Same test options as `post/3`.
  """
  @spec list(keyword()) :: [ActionLog.request()]
  def list(opts \\ []) do
    ActionLog.list_requests(Keyword.get(opts, :action_log, ActionLog))
  end

  defp kind_and_ref(opts) do
    case {Keyword.get(opts, :kind, :adhoc), Keyword.get(opts, :ref)} do
      {:adhoc, nil} ->
        {:ok, :adhoc, nil}

      {:adhoc, ref} ->
        {:error, "ref #{inspect(ref)} needs kind: :issue; an adhoc request has no ref"}

      {:issue, ref} when is_binary(ref) ->
        if Regex.match?(~r{\A[\w.-]+/[\w.-]+#\d+\z}, ref) do
          {:ok, :issue, ref}
        else
          {:error, "unrecognized issue ref #{inspect(ref)}; pass \"owner/repo#n\""}
        end

      {:issue, nil} ->
        {:error, "kind: :issue needs ref: \"owner/repo#n\""}

      {kind, _ref} ->
        {:error, "unknown kind #{inspect(kind)}; post :adhoc (default) or :issue"}
    end
  end

  defp describe(%{status: :open} = request), do: "open, posted by session #{poster(request)}"

  defp describe(%{status: :claimed} = request),
    do: "claimed by session #{claimer(request)} at #{request.claimed_at}"

  defp describe(%{status: :done} = request),
    do: "done at #{request.done_at} (claimed by session #{claimer(request)})"

  defp poster(%{poster: nil, posted_by: id}), do: "##{id || "?"}"
  defp poster(%{poster: name}), do: name

  defp claimer(%{claimer: nil, claimed_by: id}), do: "##{id || "?"}"
  defp claimer(%{claimer: name}), do: name
end
