defmodule IxMcp.MCP.Tools do
  @moduledoc """
  The MCP tool surface: exactly `exec`, `session_set_name`, and `topic_set`.
  Everything that used to be a separate tool (read, trace, restart, PR
  watching, TUI driving) is an in-language callable aliased into every cell
  (#3532); `surface_guide/0` is the one text that teaches it, shared between
  the `exec` description and the server instructions. Every tool returns
  `{:ok, text}` or `{:error, text}`; the transport wraps that in the MCP
  content envelope.
  """

  alias IxMcp.Jobs
  alias IxMcp.UTF8

  @budget_cap 120

  # One exec reply's output budget. The full output always stays in the
  # job's buffer for paging (Jobs.tail/lines/grep); only what a single reply
  # carries is capped. Why cap at all: a cell that printed a multi-megabyte
  # compiled binary rode whole into one JSON-RPC line (#3538) -- no client
  # consumes that usefully, and the agent harness's context budget is orders
  # of magnitude smaller. 64KiB is far above a legitimate reply page.
  @max_output_bytes 65_536

  @surface_guide """
  Everything beyond running code is in-language, pre-aliased in every cell:

      Jobs.tail("ab12", 20)   Jobs.await("ab12")   Jobs.cancel("ab12")   Jobs.history()
      Read.file(path)                       a file; Read.file(path, first, last) slices
                                            a 1-based inclusive line range
      Edit.replace(path, old, new)          exact-string find/replace with native
                                            Claude Code Edit semantics: raises on
                                            zero or ambiguous matches (replace_all:
                                            true replaces every one), returns a
                                            numbered snippet of the edited region
      Edit.write(path, content)             write a file, creating parent dirs;
                                            the return names created vs updated
      Ix.trace()                            stack dump of every running job's processes,
                                            taken from outside with Process.info/2
      Ix.restart()                          cancel running jobs (sparing the calling
                                            cell), restart the workspace, restore
                                            bindings from the checkpoint
      PrWatch.start(pr, cwd)                watch PR state via gh; notification on
                                            merge/close/error/timeout (optional
                                            interval and timeout args). Babysitting
                                            CI? Pair it with `gh pr checks <pr>
                                            --watch --fail-fast`: checks alone hang
                                            forever on a CONFLICTING PR (dirty PRs
                                            skip CI) and never show a merge (#3548)
      Tui.act(uri, send_keys)               drive a federated TUI resource (optional
                                            peer arg)
      TuiLocal.spawn(cmd, args)             spawn a local PTY program (vim, less, a
                                            REPL); then TuiLocal.send/2,
                                            .send_key(term, "enter" | "ctrl+c" | ...),
                                            .wait_for(term, pattern),
                                            .snapshot/1, .close/1
      Gmail.send(to, subject, body)         send mail as the signed-in user
                                            (cc:/bcc:/html:/thread: opts);
                                            Gmail.search("from:x newer_than:7d",
                                            limit: 5), Gmail.show(id),
                                            Gmail.status() for the auth state
      Agents.spawn(brief, backend: :claude | :codex | :kimi)
                                            spawn a real agent CLI as an async depth-1
                                            subagent (Fable 5 card sec 8.15.3, #3700);
                                            returns its id at once. Steer with
                                            Agents.send(id, text); observe with
                                            Agents.status(), .events(id), .report(),
                                            .graph(); block only when necessary with
                                            Agents.await(id). Finals arrive as
                                            agent_finished notifications
      Fleet.multicall(Fleet.nodes(), code)  run Elixir across the fleet's server cores
                                            (Fleet.exec_least_loaded/2 for one node)
      Api.api("tail") / Api.help(Jobs, :tail)   discovery over this whole surface

  Each cell runs in its own BEAM process, so a blocking cell never delays
  other jobs or this server -- and Ix.trace/0 and Ix.restart/0 work from a
  fresh cell even while other jobs run or wedge, so a stuck job never locks
  you out of recovery.

  Prefer Fleet whenever the work is expensive -- compiling, builds, test
  suites, large data crunching, anything that would peg this workstation for
  more than a few seconds -- and for linux-only behavior checks or work
  wanting many cores or hosts (fleet nodes are root);
  keep it local for anything touching this workstation's files, repos, or
  bindings, small low-latency evals, stateful work (remote bindings do NOT
  persist -- code ships as source strings), and darwin-specific behavior.
  Fleet.nodes() == [] means no fleet is configured (helpers return
  {:error, :no_nodes}); the remote env is minimal (shell-outs limited).
  """

  @doc "The in-language surface cheat sheet (also shipped as server instructions)."
  @spec surface_guide() :: String.t()
  def surface_guide, do: @surface_guide

  @spec list() :: [map()]
  def list do
    [
      %{
        "name" => "exec",
        "description" => """
        Run Elixir on the shared persistent workspace. Waits up to `budget`
        seconds; if the code is still running it keeps going in the background
        as a job and this returns a job handle. Bindings persist across calls:
        variables, aliases, imports, and modules you define stay defined.

        #{@surface_guide}
        Write plain Elixir, not shell. For files and data use the standard
        library directly -- File.read!/1, File.write!/2, Path.wildcard/1,
        File.ls!/1, File.stat!/1 -- instead of shelling out. To change
        an existing file, reach for Edit.replace/4 before hand-rolled
        String.replace rewrites: it fails loudly on zero or ambiguous
        matches and confirms what changed. Reserve
        System.cmd/3 for real external programs (git, nix, gh), and always
        pass its arguments as a list: System.cmd("git", ["-C", dir, "status"]).
        A subprocess's stdin is a pipe that never closes, so tools that read
        stdin when given no input path -- rg and grep especially -- hang
        forever, even with cd: set. Always pass an explicit path argument:
        System.cmd("rg", ["-n", "pat", "."], cd: dir).
        Never build shell command strings, and never use bash here-docs or
        nested quoting to pass multi-line text -- they are brittle and can
        wedge this transport. To hand multi-line text to a program, write it
        to a file with File.write!/2 and pass the path.
        """,
        "inputSchema" => %{
          "type" => "object",
          "properties" => %{
            "code" => %{"type" => "string", "description" => "Elixir source to evaluate"},
            "budget" => %{
              "type" => "number",
              "description" =>
                "Seconds to wait before backgrounding the run (cap: #{@budget_cap}s)",
              "default" => 15
            },
            "intent" => %{
              "type" => "string",
              "description" => "Short human description of what this run does (titles the run)"
            }
          },
          "required" => ["code", "intent"]
        }
      },
      %{
        "name" => "session_set_name",
        "description" =>
          "Name this connection's session. Call before acting tools; the name labels every run this server records.",
        "inputSchema" => %{
          "type" => "object",
          "properties" => %{"name" => %{"type" => "string", "minLength" => 3, "maxLength" => 80}},
          "required" => ["name"]
        }
      },
      %{
        "name" => "topic_set",
        "description" =>
          "Start a new topic. Runs fold under the topic inside the session; change it when work moves to a new phase (each call starts a fresh topic, even under a repeated name).",
        "inputSchema" => %{
          "type" => "object",
          "properties" => %{"topic" => %{"type" => "string", "minLength" => 3, "maxLength" => 80}},
          "required" => ["topic"]
        }
      }
    ]
  end

  @doc """
  Run one tool call. `action_id` is the call's pre-inserted action-log row
  (#3536): a valid exec hands it to the job it spawns, which finalizes the
  row when the eval finishes; exec's argument rejection finalizes it right
  here (no job ever owns it); every other tool leaves it to the transport,
  which finalizes with the wire outcome.
  """
  @spec call(String.t(), map(), integer() | nil) :: {:ok, String.t()} | {:error, String.t()}
  def call(name, args, action_id \\ nil)

  def call("exec", %{"code" => code} = args, action_id) when is_binary(code) do
    budget = args |> Map.get("budget", 15) |> clamp_budget()
    intent = Map.get(args, "intent")

    {summary, output} = Jobs.run(code, budget: budget, intent: intent, action_id: action_id)
    {:ok, render_run(summary, output)}
  end

  def call("exec", _args, action_id) do
    if action_id, do: IxMcp.ActionLog.finish_action(action_id, "failed", true, 0)
    {:error, "exec requires string `code`"}
  end

  def call("session_set_name", %{"name" => name}, _action_id) when is_binary(name) do
    :ok = IxMcp.Session.set_name(name)
    {:ok, "session named: #{name}"}
  end

  def call("session_set_name", _args, _action_id),
    do: {:error, "session_set_name requires string `name`"}

  def call("topic_set", %{"topic" => topic}, _action_id) when is_binary(topic) do
    :ok = IxMcp.Session.set_topic(topic)
    {:ok, "topic set: #{topic}"}
  end

  def call("topic_set", _args, _action_id), do: {:error, "topic_set requires string `topic`"}

  def call(other, _args, _action_id), do: {:error, "unknown tool: #{other}"}

  defp clamp_budget(budget) when is_number(budget) and budget > 0, do: min(budget, @budget_cap)
  defp clamp_budget(_), do: 15

  defp render_run(summary, output) do
    header =
      JSON.encode!(%{
        "job" => summary.id,
        "status" => Atom.to_string(summary.status),
        "running" => summary.running,
        "elapsed_s" => Float.round(summary.elapsed_s, 2)
      })

    sections =
      [header] ++
        diagnostics_section(summary.diagnostics) ++
        output_section(output) ++
        result_section(summary)

    Enum.join(sections, "\n")
  end

  defp diagnostics_section([]), do: []
  defp diagnostics_section(diags), do: Enum.map(diags, &("-- " <> &1))

  defp output_section(""), do: []

  defp output_section(output) when byte_size(output) <= @max_output_bytes,
    do: [String.trim_trailing(output, "\n")]

  defp output_section(output) do
    kept = UTF8.truncate(output, @max_output_bytes)

    [
      String.trim_trailing(kept, "\n"),
      "[output truncated: showing first #{byte_size(kept)} of #{byte_size(output)} bytes; " <>
        "page the full output with Jobs.lines(id, first, last) or Jobs.tail(id, n)]"
    ]
  end

  defp result_section(%{running: true}) do
    [
      "job still running; page it with Jobs.tail(id, n) / await it with Jobs.await(id) in a later exec call"
    ]
  end

  defp result_section(%{status: :done, result: rendered}), do: ["=> " <> rendered]
  defp result_section(%{result: nil}), do: []
  defp result_section(%{result: rendered}), do: [rendered]
end
