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

  alias IxMcp.Image
  alias IxMcp.Jobs
  alias IxMcp.UTF8
  alias IxMcp.Workspace

  @budget_cap 120

  # One exec reply's output budget. The full output always stays in the
  # job's buffer for paging (Jobs.tail/lines/grep); only what a single reply
  # carries is capped. Why cap at all: a cell that printed a multi-megabyte
  # compiled binary rode whole into one JSON-RPC line (#3538) -- no client
  # consumes that usefully, and the agent harness's context budget is orders
  # of magnitude smaller. 64KiB is far above a legitimate reply page.
  @max_output_bytes 65_536

  # This guide's LINE ORDER is a user-visible contract, not a style preference:
  # server.ex splices it into `instructions` and the exec tool description
  # carries it, and both readers truncate around 50 lines, so a capability below
  # the cut does not exist for its reader.
  #
  # mcp_test.exs freezes that budget already ("the exec guide names the fan-out
  # surface inside the client's truncation budget"): Agents.spawn( below guide
  # index 40, Cmd.run( and Edit.replace( below 50, each also asserted to still
  # EXIST so a vanished marker fails too. Slack is thin -- 3 lines on Cmd.run as
  # of this change -- so adding lines up here means removing the same number
  # lower down. Run that test before believing an insertion is free.
  #
  # test/ix_mcp/mcp/surface_guide_test.exs covers the part that budget test does
  # not: that the anti-Bash verdict OPENS the guide and precedes every Sh example
  # line, so a future edit cannot slide the verdict past the cut while the
  # capability ceilings still pass. Marker ceilings live in mcp_test.exs, not
  # there; two gates freezing the same numbers is how they drift apart.
  @surface_guide """
  STRONGLY PREFER exec OVER THE BASH TOOL. A Bash pipeline reports one exit
  code out of N and discards every stage's stderr, so a SILENT FALSE NEGATIVE
  is its default failure mode: `rg` exiting 2 on a broken pattern is
  byte-identical to "no matches". Sh keeps every stage's rc and stderr as data,
  argv lists never re-split on whitespace, mutations verify postconditions
  against FRESH reads rather than their own output, and watchers refuse to arm
  until shown to match a positive control and reject a negative one. Use Bash
  only for an interactive one-off a dedicated tool already covers.

      Sh.pipeline([~w(rg -n TODO .), ~w(cut -d: -f1), ~w(uniq -c)]) |> Sh.run()
      Sh.ok?(result)  EVERY stage 0, not just the last    Sh.ok!(step)  stdout, or a raise + stage table
      Sh.mutate "advance main" do <pipeline> verify <fresh reads of the world> end
      Sh.watch("v", pattern: ~r/^gate: (PASS|FAIL)/m, must_match: "gate: FAIL 3", must_not_match: "..REFUSES")
      Sh.run(Sh.cmd(~w(tee out.txt)), stdin: body)  bodies ride stdin, never argv (131072-byte word cap)

  WORKSPACES (agent isolation): exec takes workspace: "name". Every agent
  that might run concurrently with another -- every subagent, teammate, or
  parallel task -- MUST pass its own name on EVERY exec call, and keep using
  that same one. A workspace is its own persistent REPL: bindings set in one
  are invisible to the rest, so concurrent agents cannot clobber each other.
  Unnamed calls share "main"; a lone session may stay there, concurrent ones
  must not. Workspace.list()/new("name")/drop("name") manage them. Modules,
  processes, ETS and files stay BEAM-global; only bindings and env isolate.

  Everything beyond running code is in-language, pre-aliased in every cell:

      Jobs.tail("ab12", 20)   Jobs.await("ab12")   Jobs.cancel("ab12")   Jobs.history()
      Jobs.spawn(code)                      start code as a background job, returning at once
                                            (Jobs.start/2 same; Jobs.run(code, 30) waits 30s first)
      Jobs.watch("ab12") / Jobs.watch(session: 7)   announce another job's or session's
                                            finishes here (own jobs already do; Sessions.list())
      Agents.spawn(brief, backend: :claude | :codex | :kimi)
                                            FAN OUT: a real agent CLI as an async depth-1
                                            subagent, id at once. Agents.send(id, text) steers,
                                            Agents.await(id) blocks when you must; .status()
                                            .events(id) .report() .graph() observe; finals announce
      Fleet.multicall(Fleet.nodes(), code)  run Elixir across the fleet's server cores
                                            (exec_least_loaded/2 for one; topology() names hosts)
      Fleet.check()                         poll for alert conditions now; .errors non-empty means
                                            part of the fleet could NOT be read, which is NOT healthy
      Fleet.mute("anomaly")                 UNSUBSCRIBE durably; the five shapes, cadence and
                                            threshold are under FLEET at the end of this guide
      Read.file(path)                       a file; Read.file(path, first, last) slices a 1-based range
      Edit.replace(path, old, new)          exact-string find/replace, native Claude Code Edit
                                            semantics: raises on zero or ambiguous matches
                                            (replace_all: true for all), returns a numbered snippet
      Edit.write(path, content)             write a file, creating parent dirs (created vs updated)
      Cmd.run("rg", ["-n", "pat"], cd: dir) System.cmd/3 with stdin from /dev/null,
                                            so pathless rg/grep see EOF instead of
                                            hanging forever on the port's
                                            never-closing stdin pipe (#3867);
                                            Cmd.sh("rg pat | head") runs a one-line
                                            pipeline with the same EOF stdin; both
                                            default cd: to the kernel's launch dir
                                            (never the movable OS cwd, #3902), so
                                            pass cd: or -C to work elsewhere.
                                            Both return {out, exit_status} -- CHECK
                                            the status; a nonzero exit also lands
                                            as a note on the reply. Cmd.run!/
                                            Cmd.sh! raise on nonzero instead,
                                            wanted whenever failure should fail
                                            the cell. JSON needs no dep dance:
                                            Jason.decode!/1 and OTP's :json are
                                            both available in every cell
      Image.read(path)                      a PNG/JPEG/GIF/WebP as a value; when a
                                            cell's RESULT is (or contains) images,
                                            the reply carries real MCP image
                                            blocks the client renders as pictures.
                                            Image.from_binary(bytes) wraps
                                            generated bytes (charts, fetches)
      Ix.bindings()                         every bound name with the cell that bound it:
                                            job, intent, value shape, time -- scoped to
                                            the calling cell's own workspace
                                            (Ix.bindings("name") asks about another).
                                            Within one workspace a cell taking over
                                            another cell's variable is reported to
                                            both sides (#3967); across workspaces
                                            there is nothing to take over
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
      Web.search("query")                   web search, in-language, returning a
                                            LIST of %{url:, title:, text:, ...} --
                                            so filter it, pipe it into Web.fetch,
                                            keep it in a binding. results: n,
                                            text: false for URLs only
      Web.fetch("https://...")              one URL as clean text (a list of URLs
                                            gives a list of results). chars: :all
                                            lifts the per-document cap; a clipped
                                            document says so at the cut.
                                            Both run against the exa API and need
                                            EXA_API_KEY in the environment. This is
                                            the whole web surface when the session
                                            is kernelOnly -- one capability, one
                                            door
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
      Memories.search("why did every host rebuild")
                                            repo-local memory: a %Memories.Results{} of
                                            ranked `hits` plus the `roots` it resolved, one
                                            %Root{path, exists, memories} row each, over every
                                            `.memories/` directory (this repo, its parents,
                                            ~/.memories). Empty `hits` is an answer, not an
                                            error -- there is a score floor -- and the row
                                            counts say which: `memories: 0` everywhere means
                                            the search covered nothing, healthy counts mean a
                                            real miss. Ranked already, ties broken by slug: no
                                            rank step, and re-sorting loses the tie-break that
                                            makes limit: reproducible. dirs: ["/a/.memories"]
                                            replaces the default roots (a list, never a bare
                                            string), Memories.roots() resolves them with no
                                            query
      Memories.remember("slug", "tldr", body: md, by: "claude-opus-5", how: "the command")
                                            save what you learned, got wrong, or decided
                                            against. by:/how: are required for the default
                                            genre: :memory and become its first `validated`
                                            receipt, so one call writes a lint-clean file; a
                                            genre: :living page needs neither. based_on:
                                            ["path"] is what makes staleness detectable,
                                            supersedes: replaces a wrong memory instead of
                                            editing it, scope: "user:<name>" keeps one
                                            private. Memories.validate(slug, by:, how:)
                                            records a later re-check and clears staleness
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

  FLEET, in detail. Fleet.heartbeat_period() reads the baseline period and
  takes seconds to change it (default 3600, minimum 60). Hourly is measured,
  not arbitrary: 87% of minutes are non-empty, so a per-minute line costs
  ~1,250/day where hourly costs 24. Anomalies do not wait for the hour --
  they emit within a minute, ~10/day -- and Fleet.anomaly_threshold() gives
  the count this clock hour has to exceed plus the quantile it came from,
  which is what answers "why did that not fire". Fleet.mute/1 takes five
  shapes, all durable across reconnects: "heartbeat" (the hourly baseline),
  "anomaly" (the immediate out-of-band line), "digest" (both),
  "digest:warning" (keep the line, drop one category -- also error, crit,
  alert, emerg), and "<condition id>" for one discrete alert.
  Fleet.unmute/1 undoes any of them, Fleet.mutable() lists every id (the
  discrete ones come from the loaded policy catalog), and Fleet.alerts()
  shows what is muted plus what is standing. The MCP logging/setLevel
  request raises the severity floor for discrete alerts in one go.
  Fleet.watch_warnings/1 opts in to edge notifications (usually only when
  the human asks; one watcher per kernel).

  Fleet notifications (heartbeat, anomaly, discrete alerts) ride
  notifications/claude/channel, which is a Claude Code extension rather than
  standard MCP: a non-Claude MCP client receives none of them, and for those
  clients the initialize `instructions` field is the only surface that reaches
  the reader (#3785).
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
        Run Elixir on a persistent workspace (REPL). Waits up to `budget`
        seconds; if the code is still running it keeps going in the background
        as a job and this returns a job handle. Bindings persist across calls:
        variables, aliases, imports, and modules you define stay defined.

        ISOLATION RULE: if you are a subagent, a teammate, or otherwise might
        run concurrently with another agent on this connection, pass
        workspace: "<your-agent-or-task-name>" on EVERY call -- your first
        call creates it. Named workspaces are isolated REPLs; the default
        "main" workspace is shared and concurrent agents in it clobber each
        other's bindings.
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
            },
            "workspace" => %{
              "type" => "string",
              "description" =>
                "Named REPL to run in (default \"main\", which is SHARED). " <>
                  "Subagents and concurrent agents MUST pass their own name here " <>
                  "on every call; same name = same bindings, new names are " <>
                  "created on first use. Letters, digits, '.', '-', '_'."
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
  @spec call(String.t(), map(), integer() | nil) ::
          {:ok, String.t() | [map()]} | {:error, String.t()}
  def call(name, args, action_id \\ nil)

  def call("exec", %{"code" => code} = args, action_id) when is_binary(code) do
    budget = args |> Map.get("budget", 15) |> clamp_budget()
    intent = Map.get(args, "intent")
    workspace = Map.get(args, "workspace", Workspace.main())

    unless Workspace.valid_name?(workspace) do
      if action_id, do: IxMcp.ActionLog.finish_action(action_id, "failed", true, 0)

      throw(
        {:invalid_workspace,
         "invalid workspace name #{inspect(workspace)}: use letters, digits, " <>
           "'.', '-' or '_' (max 64 chars)"}
      )
    end

    {summary, output} =
      Jobs.run(code,
        budget: budget,
        intent: intent,
        action_id: action_id,
        workspace: workspace
      )

    reply = render_run(summary, output, workspace)

    # The reply just rendered carries the job's outcome, so announcing the
    # finish on the channel would say everything twice (#3934). Ack strictly
    # after the render: a death anywhere before this line leaves the outbox
    # row unacked and the durable announcement still fires -- suppression
    # must never outrun delivery. A ledger outage here degrades to a
    # duplicate announce, which beats a lost reply.
    unless summary.running, do: ack_delivered(summary.id)

    {:ok, attach_images(reply, summary)}
  catch
    {:invalid_workspace, message} -> {:error, message}
  end

  def call("exec", _args, action_id) do
    if action_id, do: IxMcp.ActionLog.finish_action(action_id, "failed", true, 0)
    {:error, "exec requires string `code`"}
  end

  def call(other, _args, _action_id), do: {:error, "unknown tool: #{other}"}

  defp clamp_budget(budget) when is_number(budget) and budget > 0, do: min(budget, @budget_cap)
  defp clamp_budget(_), do: 15

  # Design note, piggybacking announcements onto a tool reply (#3934).
  # This is the seam: after `render_run/3` and before the reply is returned,
  # `ActionLog.unacked_outbox(session)` minus this job's own row is exactly
  # the set a client never rendered, and `Notifier`'s digest renderer already
  # turns that set into one block. Acking is the easy half and is already
  # safe: `ack_outbox_ref/3` returns a claim count, so the channel path and a
  # piggyback cannot both render one row no matter which wins the race.
  #
  # What blocks it is the gate, not the drain. Draining unconditionally would
  # double every announcement for a client that DOES render the channel, so
  # the piggyback has to fire only for clients that do not, and this server
  # discards `clientInfo`/`capabilities` at `initialize` -- nothing retains
  # the negotiated `claude/channel` experimental capability, so there is
  # nothing here to branch on. Implementing it therefore means storing that
  # capability at initialize first, then reading it at this line.
  #
  # Note also what a piggyback can and cannot be: it only fires when the
  # client next calls a tool, so its delivery latency is unbounded and it is
  # a fallback for a client that never renders the channel at all, never a
  # substitute for the durable row. The row is what makes a death recoverable.
  defp ack_delivered(job_id) do
    IxMcp.ActionLog.ack_outbox_ref(:jobs, job_id)
  catch
    :exit, _reason -> 0
  end

  # A finished cell whose result value is (or contains) images answers with
  # mixed MCP content: the text verdict first, then one image block per
  # image, so the client renders the pictures instead of reading bytes. The
  # raw value lives only in the resident job process; a value the ledger
  # already flattened to text has no images to attach.
  defp attach_images(reply, %{running: false, status: :done} = summary) do
    with {:ok, value} <- Jobs.result(summary.id),
         [_at_least_one | _] = images <- Image.collect(value) do
      [%{"type" => "text", "text" => reply} | Enum.map(images, &Image.to_content/1)]
    else
      _no_images -> reply
    end
  catch
    # The result read is a GenServer.call into the job's control process,
    # which under ledger load can be parked past the call timeout (#4082).
    # Losing the image blocks then is a degraded reply; losing the whole
    # exec reply to an inherited exit would be a regression.
    :exit, _reason -> reply
  end

  defp attach_images(reply, _summary), do: reply

  defp render_run(summary, output, workspace) do
    header =
      %{
        "job" => summary.id,
        "status" => Atom.to_string(summary.status),
        "running" => summary.running,
        "elapsed_s" => Float.round(summary.elapsed_s, 2)
      }
      |> put_workspace(workspace)
      |> JSON.encode!()

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

  # "main" stays implicit so unnamed sessions read exactly what they always
  # did; a named workspace is stated on every reply, confirming which REPL
  # the bindings landed in.
  defp put_workspace(header, workspace) do
    if workspace == Workspace.main() do
      header
    else
      Map.put(header, "workspace", workspace)
    end
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
