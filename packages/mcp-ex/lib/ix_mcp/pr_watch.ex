defmodule IxMcp.PrWatch do
  @moduledoc """
  Watch a GitHub pull request via `gh` and push a channel notification when it
  merges, closes, or the watch times out -- `PrWatch.start/2..4` from any cell
  (aliased in the workspace prelude). Each watch is one supervised task:
  a hung `gh` invocation stalls that watch alone, and the notification carries
  the final state -- no hand-written polling loops in agent sessions.

  The notification is a promise, kept unconditionally: the release bakes gh's
  store path into `IX_MCP_GH` so a watch never depends on the PATH the MCP
  client launched the server with, and a watcher that crashes anyway reports
  the crash over the channel instead of vanishing (#3553).

  This watches PR state, not CI. When a green gate matters, pair it with
  `gh pr checks <pr> --watch --fail-fast`; state is the half agents forget
  (#3548): a CONFLICTING PR skips pull_request CI entirely, and another actor
  can rebase and merge underneath, so a checks-only poll waits forever on
  signals that will never arrive.

  Divergence from the Python tool, on purpose: no auto-merge arming. Watching
  is read-only here; merging stays an explicit act by the caller.
  """

  alias IxMcp.MCP.Notifier

  @spec start(String.t(), String.t(), number(), number()) ::
          {:ok, String.t()} | {:error, String.t()}
  def start(pr, cwd, interval_s \\ 15, timeout_s \\ 3600) do
    cond do
      gh() == nil ->
        {:error, "gh not found (no IX_MCP_GH, none on PATH); PrWatch needs the GitHub CLI"}

      not File.dir?(cwd) ->
        {:error, "cwd does not exist: #{cwd}"}

      true ->
        {:ok, pid} =
          Task.Supervisor.start_child(IxMcp.PrWatch.Supervisor, fn ->
            deadline = System.monotonic_time(:millisecond) + round(timeout_s * 1000)
            watch_loudly(pr, cwd, round(interval_s * 1000), deadline)
          end)

        {:ok, "watching PR #{pr} (#{inspect(pid)}); a channel notification reports the outcome"}
    end
  end

  # The release wrapper bakes the GitHub CLI's store path into IX_MCP_GH, so
  # a watch never depends on whatever PATH the MCP client launched the server
  # with (#3553: gh lived outside the release and the watcher died at the
  # first poll). The PATH lookup remains only for mix test / IEx runs outside
  # the release.
  defp gh do
    System.get_env("IX_MCP_GH") || System.find_executable("gh")
  end

  # start/4 promised the caller a notification, so a crashing watcher must
  # not vanish into the supervisor's logs (#3553): deliver the crash over the
  # channel first, then re-raise so the crash report still reaches the logger.
  defp watch_loudly(pr, cwd, interval_ms, deadline) do
    watch(pr, cwd, interval_ms, deadline)
  catch
    kind, reason ->
      notify(pr, "error", String.slice(Exception.format(kind, reason, __STACKTRACE__), 0, 800))
      :erlang.raise(kind, reason, __STACKTRACE__)
  end

  defp watch(pr, cwd, interval_ms, deadline) do
    case poll(pr, cwd) do
      {:final, state, detail} ->
        notify(pr, state, detail)

      :pending ->
        if System.monotonic_time(:millisecond) > deadline do
          notify(pr, "timeout", "watch window elapsed before the PR reached a final state")
        else
          Process.sleep(interval_ms)
          watch(pr, cwd, interval_ms, deadline)
        end

      {:error, detail} ->
        notify(pr, "error", detail)
    end
  end

  defp poll(pr, cwd) do
    case System.cmd(gh(), ["pr", "view", pr, "--json", "state,mergedAt,url"],
           cd: cwd,
           stderr_to_stdout: true
         ) do
      {out, 0} ->
        case JSON.decode(out) do
          {:ok, %{"state" => "MERGED"} = view} -> {:final, "merged", view["url"]}
          {:ok, %{"state" => "CLOSED"} = view} -> {:final, "closed", view["url"]}
          {:ok, _view} -> :pending
          {:error, _} -> {:error, "unparseable gh output: #{String.slice(out, 0, 200)}"}
        end

      {out, _nonzero} ->
        {:error, String.slice(out, 0, 400)}
    end
  end

  defp notify(pr, state, detail) do
    Notifier.channel(
      "PR ##{pr} #{state}: #{detail}",
      %{"source" => "pr_watch", "pr" => pr, "state" => state, "level" => level(state)}
    )
  end

  # "error" rode as "warning" before #3553; a watch that will never report
  # again is an error, not something to skim past.
  defp level("merged"), do: "info"
  defp level("error"), do: "error"
  defp level(_state), do: "warning"
end
