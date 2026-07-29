defmodule IxMcp.MCP.Tools do
  @moduledoc """
  The MCP tool surface: exactly `exec`. Everything that used to be a
  separate tool (read, trace, restart, PR
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
      Jobs.watch("ab12") / Jobs.watch(session: 7)   announce another job's or
                                            session's finishes here (own jobs
                                            already announce; session ids:
                                            Sessions.list())
      Read.file(path)                       a file; Read.file(path, first, last) slices
                                            a 1-based inclusive line range
      Edit.replace(path, old, new)          exact-string find/replace with native
                                            Claude Code Edit semantics: raises on
                                            zero or ambiguous matches (replace_all:
                                            true replaces every one), returns a
                                            numbered snippet of the edited region
      Edit.write(path, content)             write a file, creating parent dirs;
                                            the return names created vs updated
      Cmd.run("rg", ["-n", "pat"], cd: dir) System.cmd/3 with stdin from /dev/null,
                                            so pathless rg/grep see EOF instead of
                                            hanging forever on the port's
                                            never-closing stdin pipe (#3867);
                                            Cmd.sh("rg pat | head") runs a one-line
                                            pipeline with the same EOF stdin; both
                                            default cd: to the kernel's launch dir
                                            (never the movable OS cwd, #3902), so
                                            pass cd: or -C to work elsewhere
      Ix.bindings()                         every bound name with the cell that bound it:
                                            job, intent, value shape, time. This kernel's
                                            bindings are shared by every agent riding its
                                            connection -- a session and its subagents
                                            alike -- so a cell taking over another cell's
                                            variable is reported to both sides (#3967)
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
                                            skip CI) and never show a merge (#3548).
                                            Issues need no watcher: every issue
                                            filed in a watched org (default
                                            indexable-inc) arrives unprompted as a
                                            source="issues" channel notification
      Issues.pickup(3880)                   claim an issue atomically BEFORE working
                                            it (also "owner/repo#n"); won claims
                                            assign @me on GitHub and announce as
                                            source="requests"; a lost claim returns
                                            {:error, "claimed by session ..."} --
                                            pick a different issue
      Requests.post("review PR #42", body)  offer any unit of work to every agent
                                            on this host; it announces as a
                                            source="requests" channel event.
                                            Requests.pickup(id) claims one
                                            atomically BEFORE working it,
                                            Requests.done(id) when finished,
                                            Requests.list() shows the board
                                            (open first)
      Sessions.list()                       the session directory: every kernel
                                            instance on this host with its name,
                                            topic, and heartbeat liveness -- check
                                            who else is working before delegating
                                            or duplicating another session's work
      Sessions.send(12, "text")             message another session by directory id
                                            (or unique live name); it lands in that
                                            agent's context as a source="sessions"
                                            channel event, sender named, within
                                            seconds. Sessions.broadcast("text")
                                            reaches every session on the host
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
      Imsg.send(handle, text)               send an iMessage via Messages.app,
                                            and post a local banner saying so;
                                            italic:/bold:/underline:/strike: set
                                            real formatting by driving the UI;
                                            Imsg.chats(), .recent(with: handle),
                                            .search(q) read the local chat.db
                                            (own sends have NULL text; helpers
                                            decode the typedstream body)
      Contacts.search(name)                 macOS address book: name -> the
                                            phone/email handles Imsg takes
      Ask.user("Redesign or patch?", options: ["Redesign", {"Patch", "keep shape"}])
                                            put one question to the human as a
                                            native dialog (MCP elicitation);
                                            blocks the cell, returns {:ok, answer}
                                            | :declined | :cancelled | :timeout.
                                            Omit options: for free text
      Memory.remember("slug", "hook", topic: "nix")
                                            durable memory: append facts to the
                                            weave store named by
                                            WEAVE_MEMORY_STORE (loud error when
                                            unset); supersedes:/relates: opts
                                            write typed edges Memory.graph(slug)
                                            walks. Recall with
                                            Memory.recall("regex") -- whole-word
                                            rows with id, time, type, topic,
                                            handle, body -- with
                                            Memory.semantic("question") -- the
                                            same rows plus similarity, ranked by
                                            embedding when no word matches --
                                            or raw Datalog via
                                            Memory.query/1. Memory.verify(slug)
                                            records a re-check receipt;
                                            Memory.retract(id) kills a wrong
                                            fact, newer facts win over stale ones
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
        Jobs are durable: a backgrounded run's output, final status, and
        history survive even a crash or kill, readable later with
        Jobs.tail(id) / Jobs.output(id) / Jobs.history(). A backgrounded
        run's finish is announced on the channel to this session -- one
        line for a clean finish, the reason for a failure or death, several
        finishes coalesced into one digest; a result this reply already
        carried is never re-announced. Follow another session's work with
        Jobs.watch.

        #{@surface_guide}
        Write plain Elixir, not shell. For files and data use the standard
        library directly -- File.read!/1, File.write!/2, Path.wildcard/1,
        File.ls!/1, File.stat!/1 -- instead of shelling out. To change
        an existing file, reach for Edit.replace/4 before hand-rolled
        String.replace rewrites: it fails loudly on zero or ambiguous
        matches and confirms what changed. Reserve
        subprocesses for real external programs (git, nix, gh), spawn them
        with Cmd.run/3, and always pass arguments as a list:
        Cmd.run("git", ["-C", dir, "status"]). Cmd is System.cmd/3 with
        stdin redirected from /dev/null: a raw System.cmd subprocess gets a
        stdin pipe that never closes, so tools that fall back to reading
        stdin when given no input path -- rg and grep especially -- hang
        forever, even with cd: set (#3867). Cmd.sh("rg pat | head") runs a
        one-line pipeline with the same EOF stdin. Without cd: both run in
        the kernel's launch directory, never the OS cwd -- File.cd!/1
        cannot aim a pathless git at another session's checkout (#3902) --
        so name the directory with cd: (or git -C) when working elsewhere.
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
    reply = render_run(summary, output)

    # The reply just rendered carries the job's outcome, so announcing the
    # finish on the channel would say everything twice (#3934). Ack strictly
    # after the render: a death anywhere before this line leaves the outbox
    # row unacked and the durable announcement still fires -- suppression
    # must never outrun delivery. A ledger outage here degrades to a
    # duplicate announce, which beats a lost reply.
    unless summary.running, do: ack_delivered(summary.id)

    {:ok, reply}
  end

  def call("exec", _args, action_id) do
    if action_id, do: IxMcp.ActionLog.finish_action(action_id, "failed", true, 0)
    {:error, "exec requires string `code`"}
  end

  def call(other, _args, _action_id), do: {:error, "unknown tool: #{other}"}

  defp clamp_budget(budget) when is_number(budget) and budget > 0, do: min(budget, @budget_cap)
  defp clamp_budget(_), do: 15

  defp ack_delivered(job_id) do
    IxMcp.ActionLog.ack_job_outbox(job_id)
  catch
    :exit, _reason -> 0
  end

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
