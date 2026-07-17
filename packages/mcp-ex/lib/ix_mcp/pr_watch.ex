defmodule IxMcp.PrWatch do
  @moduledoc """
  Watch a GitHub pull request via `gh` and push a channel notification when it
  merges, closes, or the watch times out. Each watch is one supervised task:
  a hung `gh` invocation stalls that watch alone, and the notification carries
  the final state -- no hand-written polling loops in agent sessions.

  Divergence from the Python tool, on purpose: no auto-merge arming. Watching
  is read-only here; merging stays an explicit act by the caller.
  """

  alias IxMcp.MCP.Notifier

  @spec start(String.t(), String.t(), number(), number()) ::
          {:ok, String.t()} | {:error, String.t()}
  def start(pr, cwd, interval_s, timeout_s) do
    cond do
      System.find_executable("gh") == nil ->
        {:error, "gh not found on PATH; pr_watch needs the GitHub CLI"}

      not File.dir?(cwd) ->
        {:error, "cwd does not exist: #{cwd}"}

      true ->
        {:ok, pid} =
          Task.Supervisor.start_child(IxMcp.PrWatch.Supervisor, fn ->
            deadline = System.monotonic_time(:millisecond) + round(timeout_s * 1000)
            watch(pr, cwd, round(interval_s * 1000), deadline)
          end)

        {:ok, "watching PR #{pr} (#{inspect(pid)}); a channel notification reports the outcome"}
    end
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
    case System.cmd("gh", ["pr", "view", pr, "--json", "state,mergedAt,url"],
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
    Notifier.notify("notifications/message", %{
      "level" => if(state == "merged", do: "info", else: "warning"),
      "logger" => "ix_mcp.pr_watch",
      "data" => %{"event" => "pr_watch", "pr" => pr, "state" => state, "detail" => detail}
    })
  end
end
