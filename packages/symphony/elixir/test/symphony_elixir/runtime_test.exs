defmodule SymphonyElixir.RuntimeTest do
  use ExUnit.Case, async: false

  # The #90 crash tests deliberately kill executor tasks, which logs the
  # crash and the deadlock-guard error. Capture it so a passing run stays
  # quiet; a real regression still surfaces through the assertions.
  @moduletag capture_log: true

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.{Materializer, Node, RunGraph, Store}
  alias SymphonyElixir.Runtime

  # A fake EngineClient driven by a per-test ETS table mapping a node id
  # to an instruction. The table name is fixed but rows are cleared in
  # setup, so `async: false` keeps tests from racing each other.
  defmodule FakeEngine do
    @behaviour SymphonyElixir.Runtime.EngineClient

    @table :runtime_test_fake

    def setup do
      if :ets.whereis(@table) == :undefined do
        :ets.new(@table, [:named_table, :public, :set])
      end

      :ets.delete_all_objects(@table)
      :ok
    end

    # `instruction` is one of:
    #   {:ok, output}        -> succeed with output
    #   {:error, reason}     -> fail
    #   {:ok, output, tid}   -> succeed and report thread id
    #   :crash               -> raise, so the task dies without :node_done
    #   {:sleep_then, instr} -> sleep so the test can observe :running first
    def program(node_id, instruction), do: :ets.insert(@table, {node_id, instruction})

    def set_status(thread_id, status), do: :ets.insert(@table, {{:status, thread_id}, status})

    # The run_opts a node's turn was invoked with, so a test can assert the
    # runtime threaded the resolved working directory in.
    def opts_for(node_id) do
      case :ets.lookup(@table, {:opts, node_id}) do
        [{_, opts}] -> opts
        [] -> nil
      end
    end

    @impl true
    def run_node(%Node{id: id}, opts) do
      :ets.insert(@table, {{:opts, id}, opts})

      case lookup(id) do
        {:ok, output} -> {:ok, output, nil}
        {:ok, output, tid} -> {:ok, output, tid}
        {:error, reason} -> {:error, reason, nil}
        :crash -> raise "fake engine crash for #{id}"
        {:sleep_then, instr} -> sleep_then(id, instr)
        nil -> {:ok, %{default: id}, nil}
      end
    end

    @impl true
    def status(thread_id) do
      case :ets.lookup(@table, {:status, thread_id}) do
        [{_, status}] -> status
        [] -> :unknown
      end
    end

    defp sleep_then(id, instr) do
      Process.sleep(50)
      :ets.insert(@table, {id, instr})
      run_node(%Node{id: id, ast_origin: nil, kind: :exec, inputs: %{}, deps: [], state: :running}, %{})
    end

    defp lookup(id) do
      case :ets.lookup(@table, id) do
        [{^id, instruction}] -> instruction
        [] -> nil
      end
    end
  end

  # A placement double that resolves a fixed working directory, so a test
  # can assert the runtime threads the checkout path into an agent turn
  # without provisioning a real room-server.
  defmodule CwdPlacement do
    def acquire(_run_id, _location, _opts), do: {:ok, "http://stub.test"}
    def resolved(_run_id), do: {:ok, %{location: :host, base_url: "http://stub.test"}}
    def workspace_cwd(_run_id, _opts), do: {:ok, "/checkout/run/example"}
    def release(_run_id), do: :ok
  end

  # A placement double that forwards the opts `acquire/3` received to the
  # test process (the `:test_pid` is threaded through `placement_opts`), so a
  # test can assert the runtime minted and passed a GitHub App `:bot_token`.
  defmodule RecordingPlacement do
    def acquire(_run_id, _location, opts) do
      if pid = Keyword.get(opts, :test_pid), do: send(pid, {:acquire_opts, opts})
      {:ok, "http://stub.test"}
    end

    def resolved(_run_id), do: {:ok, %{location: :host, base_url: "http://stub.test"}}
    def workspace_cwd(_run_id, _opts), do: {:ok, "/checkout/run/example"}
    def release(_run_id), do: :ok
  end

  setup do
    FakeEngine.setup()
    start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})

    dir = Path.join(System.tmp_dir!(), "runtime_test_#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    on_exit(fn -> File.rm_rf(dir) end)
    {:ok, dir: dir}
  end

  # The subrun tests launch a nested run through Runtime.Ingress, which
  # resolves the child workflow through WorkflowCatalog's ETS table and
  # starts it under Runtime.Supervisor. Create the table and the supervisor
  # only when a test needs them so the rest of the suite stays untouched.
  defp ensure_subrun_substrate do
    table = :symphony_workflows

    if :ets.whereis(table) == :undefined do
      :ets.new(table, [:named_table, :public, read_concurrency: true])
    else
      :ets.delete_all_objects(table)
    end

    unless Process.whereis(SymphonyElixir.Runtime.Supervisor) do
      start_supervised!(SymphonyElixir.Runtime.Supervisor)
    end

    :ok
  end

  defp put_workflow(name, source) do
    {:ok, ast} = SymphonyElixir.DSL.Parser.parse(source)
    entry = %{name: ast.name || name, ast: ast, trigger: ast.trigger, source: source, hash: :crypto.hash(:sha256, source)}
    :ets.insert(:symphony_workflows, {name, entry})
  end

  # Agent nodes by default so each attempt routes through the injected
  # FakeEngine; exec nodes run locally and would bypass it. A test can still
  # pass `kind:`/`envelope:` to override.
  defp node(id, opts) do
    base = [
      id: id,
      ast_origin: {:t, id},
      kind: Keyword.get(opts, :kind, :agent),
      envelope: Keyword.get(opts, :envelope, %Envelope{engine: :codex, model: "m"}),
      inputs: Keyword.get(opts, :inputs, %{})
    ]

    Node.new(base ++ Keyword.take(opts, [:state, :attempts]))
  end

  defp graph(run_id, nodes), do: RunGraph.new(run_id, "h", {:ast, []}) |> RunGraph.put_nodes(nodes)

  # Materialize a `.sym` source into a real RunGraph so the runtime drives
  # the AST through `Materializer.expand_dynamic/1` on each success. The
  # gate tests need the AST present (the hand-built `graph/2` carries a
  # placeholder `{:ast, []}` that re-expands to nothing); this gives the
  # supervised run an actual `when`/`every` construct to resolve.
  defp materialized(run_id, source) do
    {:ok, ast} = SymphonyElixir.DSL.Parser.parse(source)
    {:ok, graph} = Materializer.materialize(run_id, "h", ast)
    graph
  end

  defp opts(dir), do: [engine: FakeEngine, store_opts: [dir: dir]]

  # A run settles when the GenServer stops (succeeded/cancelled) or stays
  # alive and idle on a terminal :failed status (WS-6 keeps a failed run
  # alive so the operator surface can reach it). Treat both as settled.
  defp wait_for_exit(pid) do
    ref = Process.monitor(pid)

    receive do
      {:DOWN, ^ref, :process, ^pid, _} -> :ok
    after
      0 -> wait_for_settled(pid, ref)
    end
  end

  defp wait_for_settled(pid, ref, attempts \\ 100) do
    receive do
      {:DOWN, ^ref, :process, ^pid, _} -> :ok
    after
      20 ->
        cond do
          settled_failed?(pid) ->
            Process.demonitor(ref, [:flush])
            :ok

          attempts == 0 ->
            flunk("runtime did not settle in time")

          true ->
            wait_for_settled(pid, ref, attempts - 1)
        end
    end
  end

  defp settled_failed?(pid) do
    Process.alive?(pid) and SymphonyElixir.Runtime.graph(pid).status == :failed
  catch
    :exit, _ -> true
  end

  test "runs a linear two-node graph to success", %{dir: dir} do
    g =
      graph("run-linear", [
        node("a", state: :pending),
        node("b", state: :pending, inputs: %{"x" => {:node, "a", []}})
      ])

    FakeEngine.program("a", {:ok, %{v: 1}})
    FakeEngine.program("b", {:ok, %{v: 2}})

    {:ok, pid} = Runtime.start_link(g, opts(dir))
    wait_for_exit(pid)

    {:ok, final} = Store.load("run-linear", dir: dir)
    assert final.status == :succeeded
    assert final.nodes["a"].state == :succeeded
    assert final.nodes["b"].state == :succeeded
  end

  test "a stray message does not crash the run", %{dir: dir} do
    g = graph("run-stray-message", [node("a", state: :pending)])
    FakeEngine.program("a", {:sleep_then, {:ok, %{v: 1}}})

    {:ok, pid} = Runtime.start_link(g, opts(dir))

    # The shape a timed-out Command.run child used to leak: a raw port
    # line dequeued by the GenServer long after collect/4 gave up.
    # Without the catch-all clause this was a FunctionClauseError that
    # killed the run mid-flight (and the transient restart then
    # double-submitted the in-flight turn).
    send(pid, {self(), {:data, "Interrupted. Shutting down...\n"}})

    wait_for_exit(pid)

    {:ok, final} = Store.load("run-stray-message", dir: dir)
    assert final.status == :succeeded
    assert final.nodes["a"].state == :succeeded
  end

  test "threads the resolved placement cwd into an agent turn", %{dir: dir} do
    # A `{:host, _}` location makes the runtime acquire a placement, so the
    # agent run_opts must carry the checkout cwd the engine turn needs.
    envelope = %Envelope{engine: :codex, model: "m", location: {:host, "box"}}
    g = graph("run-cwd", [node("a", state: :pending, envelope: envelope)])

    FakeEngine.program("a", {:ok, %{v: 1}})

    {:ok, pid} = Runtime.start_link(g, engine: FakeEngine, placement: CwdPlacement, store_opts: [dir: dir])
    wait_for_exit(pid)

    assert FakeEngine.opts_for("a")[:cwd] == "/checkout/run/example"
  end

  test "mints a GitHub App token and threads it into placement acquire", %{dir: dir} do
    # With a GitHub App configured, the runtime must pass a freshly minted
    # installation token as `:bot_token` so the workspace clone auth and the
    # room-server GITHUB_TOKEN/GH_TOKEN author agent PRs under the App's bot
    # identity rather than the static host token (ENG-2012,
    # indexable-inc/symphony#242).
    snapshot = SymphonyElixir.Config.get()

    :ets.insert(
      :symphony_config,
      {:snapshot, %{snapshot | github_app_id: "123", github_app_private_key_pem: "PEM"}}
    )

    on_exit(fn -> :ets.insert(:symphony_config, {:snapshot, snapshot}) end)

    # Seed the installation-token cache so `GithubApp.installation_token/0`
    # answers without the GenServer (unstarted in this test) or a real mint.
    if :ets.whereis(:symphony_github_app_token) == :undefined do
      :ets.new(:symphony_github_app_token, [:named_table, :public, read_concurrency: true])
    end

    :ets.insert(
      :symphony_github_app_token,
      {:current, %{token: "app-token", expires_at: DateTime.add(DateTime.utc_now(), 3600, :second), installation_id: 1}}
    )

    # The seeded table is owned by this test process when GithubApp is not
    # supervised (it vanishes on exit); only drop the entry if a real,
    # longer-lived table is present so the seed cannot leak into other tests.
    on_exit(fn ->
      if :ets.whereis(:symphony_github_app_token) != :undefined do
        :ets.delete(:symphony_github_app_token, :current)
      end
    end)

    envelope = %Envelope{engine: :codex, model: "m", location: {:host, "box"}}
    g = graph("run-bot-token", [node("a", state: :pending, envelope: envelope)])

    FakeEngine.program("a", {:ok, %{v: 1}})

    {:ok, pid} =
      Runtime.start_link(g,
        engine: FakeEngine,
        placement: RecordingPlacement,
        store_opts: [dir: dir, placement_opts: [test_pid: self()]]
      )

    wait_for_exit(pid)

    assert_received {:acquire_opts, opts}
    assert Keyword.get(opts, :bot_token) == "app-token"
  end

  test "threads a successful turn's cost onto the recorded attempt", %{dir: dir} do
    g = graph("run-cost", [node("a", state: :pending)])

    cost = %{usd: 0.0123, tokens_in: 1200, tokens_out: 340, cache_read: 800, cache_creation: 64}
    FakeEngine.program("a", {:ok, %{thread_id: "thread_abc", event_count: 4, cost: cost}, "thread_abc"})

    {:ok, pid} = Runtime.start_link(g, opts(dir))
    wait_for_exit(pid)

    {:ok, final} = Store.load("run-cost", dir: dir)
    assert final.status == :succeeded
    [attempt] = final.nodes["a"].attempts
    assert attempt.state == :succeeded
    assert attempt.cost == cost
  end

  test "runs parallel-ready siblings concurrently", %{dir: dir} do
    g =
      graph("run-parallel", [
        node("a", state: :pending),
        node("b", state: :pending)
      ])

    FakeEngine.program("a", {:ok, :ok})
    FakeEngine.program("b", {:ok, :ok})

    {:ok, pid} = Runtime.start_link(g, opts(dir))
    wait_for_exit(pid)

    {:ok, final} = Store.load("run-parallel", dir: dir)
    assert final.status == :succeeded
  end

  test "a node failure propagates upstream_failed and the run fails", %{dir: dir} do
    g =
      graph("run-fail", [
        node("a", state: :pending),
        node("b", state: :pending, inputs: %{"x" => {:node, "a", []}})
      ])

    FakeEngine.program("a", {:error, :boom})

    {:ok, pid} = Runtime.start_link(g, opts(dir))
    wait_for_exit(pid)

    {:ok, final} = Store.load("run-fail", dir: dir)
    assert final.status == :failed
    assert final.nodes["a"].state == :failed
    assert final.nodes["b"].state == :upstream_failed
  end

  describe "#90: executor task dies without :node_done" do
    test "a crashing task strands the node and the run resolves (no opt-in retry)", %{dir: dir} do
      g = graph("run-crash", [node("a", state: :pending)])
      FakeEngine.program("a", :crash)

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-crash", dir: dir)
      # The node cannot run again without operator action; the run does not
      # hang. With no remaining ready work the deadlock guard resolves it.
      assert final.nodes["a"].state == :stranded
      assert final.status == :failed
      [att] = final.nodes["a"].attempts
      assert att.state == :stranded
    end

    test "an opted-in node with no side effect auto-retries after a crash", %{dir: dir} do
      g = graph("run-retry", [node("a", state: :pending, inputs: %{"__retry__" => {:literal, true}})])

      # First attempt crashes; the retry succeeds. The fake flips the
      # instruction the first time it is asked to crash.
      FakeEngine.program("a", :crash)

      test_pid = self()

      # Replace the crash with a success once the strand has been recorded.
      spawn(fn ->
        Process.sleep(80)
        FakeEngine.program("a", {:ok, :recovered})
        send(test_pid, :reprogrammed)
      end)

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-retry", dir: dir)
      assert final.nodes["a"].state in [:succeeded, :stranded]
    end
  end

  describe "#90: deadlock guard" do
    test "a graph with no ready nodes and no tasks fails instead of hanging", %{dir: dir} do
      # `a` depends on a node that never succeeds (it is itself blocked by a
      # missing dep id), so no node is ever ready.
      g =
        graph("run-deadlock", [
          node("a", state: :pending, inputs: %{"x" => {:node, "ghost", []}})
        ])

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-deadlock", dir: dir)
      assert final.status == :failed
    end
  end

  describe "#90: BEAM restart reconciliation" do
    test "a persisted :running node makes progress after a simulated restart", %{dir: dir} do
      # Persist a graph as if the BEAM died mid-turn: node `a` is :running
      # with an attempt that opened no thread, and `b` waits on it.
      attempt = SymphonyElixir.IR.Attempt.start(1, :codex, nil)

      g =
        graph("run-restart", [
          node("a", state: :running, attempts: [attempt], inputs: %{"__retry__" => {:literal, true}}),
          node("b", state: :pending, inputs: %{"x" => {:node, "a", []}})
        ])

      :ok = Store.persist(g, dir: dir)

      # On restart the engine cannot account for the thread (no thread id),
      # so reconcile auto-retries `a` (opted in, no side effect). The rerun
      # then succeeds and unblocks `b`.
      FakeEngine.program("a", {:ok, :ok})
      FakeEngine.program("b", {:ok, :ok})

      {:ok, reloaded} = Store.load("run-restart", dir: dir)
      {:ok, pid} = Runtime.start_link(reloaded, [recover: true] ++ opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-restart", dir: dir)
      assert final.nodes["a"].state == :succeeded
      assert final.nodes["b"].state == :succeeded
      assert final.status == :succeeded
    end

    test "a persisted :running node with an opened thread strands on restart", %{dir: dir} do
      attempt = SymphonyElixir.IR.Attempt.start(1, :codex, "thread-x")
      g = graph("run-restart-strand", [node("a", state: :running, attempts: [attempt])])
      :ok = Store.persist(g, dir: dir)

      # status :unknown -> the thread cannot be accounted for; a recorded
      # thread id means a side effect may have happened, so strand.
      {:ok, reloaded} = Store.load("run-restart-strand", dir: dir)
      {:ok, pid} = Runtime.start_link(reloaded, [recover: true] ++ opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-restart-strand", dir: dir)
      assert final.nodes["a"].state == :stranded
      assert final.status == :failed
    end
  end

  describe "PubSub: live transitions broadcast" do
    alias SymphonyElixir.Runtime.Events

    # The app PubSub (`SymphonyElixir.PubSub`) is started once in
    # `test_helper.exs`, so a subscriber here receives the runtime's
    # broadcasts without booting any extra process.

    test "a subscriber receives an event for each persisted transition", %{dir: dir} do
      g =
        graph("run-pubsub", [
          node("a", state: :pending),
          node("b", state: :pending, inputs: %{"x" => {:node, "a", []}})
        ])

      FakeEngine.program("a", {:ok, %{v: 1}})
      FakeEngine.program("b", {:ok, %{v: 2}})

      :ok = Events.subscribe_run("run-pubsub")
      :ok = Events.subscribe_index()

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      summaries = drain_events("run-pubsub")

      # Each persisted transition announces, so a two-node run that marks
      # each node running then succeeded fans out several events, not one.
      assert length(summaries) > 1

      # The run resolves succeeded, so the last announced summary carries
      # the terminal status and both nodes counted as succeeded.
      last = List.last(summaries)
      assert last["status"] == "succeeded"
      assert last["states"] == %{"succeeded" => 2}

      # An intermediate transition is observable: at least one summary shows
      # a node already succeeded while the run had not yet finished, proving
      # the page would update before the run completes.
      assert Enum.any?(summaries, fn s -> s["states"]["succeeded"] == 1 end)

      # The per-run and index topics carry the same message, so the
      # subscriber sees each transition twice (once per topic). Both shapes
      # are the `IR.View.summary/1` map keyed on this run.
      assert Enum.all?(summaries, &match?(%{"run_id" => "run-pubsub"}, &1))
    end

    # Collect every `{:ir_run_event, run_id, summary}` currently in the
    # mailbox for one run. The subscriber is registered on both topics, so
    # this drains the duplicate index + per-run deliveries too.
    defp drain_events(run_id, acc \\ []) do
      receive do
        {:ir_run_event, ^run_id, summary} -> drain_events(run_id, [summary | acc])
      after
        50 -> Enum.reverse(acc)
      end
    end
  end

  describe "subrun: nested child runs" do
    # A child workflow with a single agent node. Its node id is
    # content-derived, so the test does not program the FakeEngine for it;
    # the fake's default branch succeeds any unprogrammed node, which is
    # enough to drive the child to a :succeeded terminal status.
    @child_sym ~s|workflow "child" on manual { c <- agent { engine: codex, model: "m", prompt: inline "do" } }|

    test "a subrun starts a child run and its terminal output flows to the parent", %{dir: dir} do
      ensure_subrun_substrate()
      put_workflow("child", @child_sym)

      g =
        graph("run-subrun-ok", [
          Node.new(
            id: "s",
            ast_origin: {:t, "s"},
            kind: :subrun,
            inputs: %{"source" => {:literal, "child.sym"}},
            state: :pending
          )
        ])

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-subrun-ok", dir: dir)
      assert final.status == :succeeded
      assert final.nodes["s"].state == :succeeded

      # The subrun node's output names the child run and carries its status,
      # so a downstream node could read the child result through its inputs.
      output = final.nodes["s"].output
      assert output.kind == :subrun
      assert output.status == :succeeded
      assert is_binary(output.run_id)

      # The attempt records the subrun executor, not a sham engine.
      [attempt] = final.nodes["s"].attempts
      assert attempt.engine == :subrun
      assert attempt.state == :succeeded

      # The child run was persisted under its own id in the shared store.
      assert {:ok, child} = Store.load(output.run_id, dir: dir)
      assert child.status == :succeeded
    end

    test "a self-referential subrun is rejected as a cycle without spawning a child", %{dir: dir} do
      ensure_subrun_substrate()
      put_workflow("child", @child_sym)

      g =
        graph("run-subrun-cycle", [
          Node.new(
            id: "s",
            ast_origin: {:t, "s"},
            kind: :subrun,
            inputs: %{"source" => {:literal, "child.sym"}},
            state: :pending
          )
        ])

      # The parent is itself a "child" run already (its name is on the
      # ancestor chain), so a subrun back to "child" closes a cycle.
      sub_opts = opts(dir) ++ [subrun_ancestors: ["child"]]
      {:ok, pid} = Runtime.start_link(g, sub_opts)
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-subrun-cycle", dir: dir)
      assert final.status == :failed
      assert final.nodes["s"].state == :failed
      assert {:error, {:subrun_cycle, "child", ["child"]}} = final.nodes["s"].output
    end

    test "a subrun over the depth ceiling is rejected", %{dir: dir} do
      ensure_subrun_substrate()
      put_workflow("child", @child_sym)

      g =
        graph("run-subrun-depth", [
          Node.new(
            id: "s",
            ast_origin: {:t, "s"},
            kind: :subrun,
            inputs: %{"source" => {:literal, "child.sym"}},
            state: :pending
          )
        ])

      # Start already at the ceiling so the child (depth + 1) trips the cap.
      ceiling = SymphonyElixir.Config.get().subrun_max_depth
      sub_opts = opts(dir) ++ [subrun_depth: ceiling]
      {:ok, pid} = Runtime.start_link(g, sub_opts)
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-subrun-depth", dir: dir)
      assert final.status == :failed
      assert final.nodes["s"].state == :failed
      assert {:error, {:subrun_depth_exceeded, _depth, ^ceiling}} = final.nodes["s"].output
    end
  end

  describe "when/every gate execution (Phase 7)" do
    # A gating agent followed by a `when ${a.changed}` body agent. The
    # interpreter ids are content-derived: the gating agent is `agent-0`,
    # the gate placeholder is `when-1`, and the body agent that the firing
    # pass emits is `agent-2`. The supervised run must drive `agent-0` to
    # success, re-expand on its output, then schedule (or skip) `agent-2`.
    @when_sym ~s|workflow "gate" on manual { a <- agent { engine: codex, model: "m", prompt: inline "decide" } when ${a.changed} { b <- agent { engine: codex, model: "m", prompt: inline "act" } } }|

    test "a when gate that resolves true runs the gated body under a supervised run", %{dir: dir} do
      g = materialized("run-when-true", @when_sym)

      # The gate reads `${a.changed}`; the body agent is unprogrammed and
      # falls through the fake's default success. Atom-keyed output is fine:
      # the interpreter's field read digs string or atom keys.
      FakeEngine.program("agent-0", {:ok, %{changed: true}})

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-when-true", dir: dir)
      assert final.status == :succeeded
      assert final.nodes["agent-0"].state == :succeeded
      # The gated body materialized after agent-0 succeeded and ran to success.
      assert final.nodes["agent-2"].state == :succeeded
      # The resolved gate placeholder was retired so it did not deadlock the run.
      assert final.nodes["when-1"].state == :skipped
    end

    test "a when gate that resolves false skips the body and the run still succeeds", %{dir: dir} do
      g = materialized("run-when-false", @when_sym)

      FakeEngine.program("agent-0", {:ok, %{changed: false}})

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-when-false", dir: dir)
      assert final.status == :succeeded
      assert final.nodes["agent-0"].state == :succeeded
      # The body was never emitted: a falsy gate produces no child node.
      refute Map.has_key?(final.nodes, "agent-2")
      # The placeholder is retired to :skipped, the load-bearing pair with
      # the deadlock guard so a never-fired gate does not stall the run.
      assert final.nodes["when-1"].state == :skipped
    end

    # `every n of c { ... }` is an interpreter gate keyed on the persisted
    # expansion log, not a wall-clock schedule. In a single run the gate is
    # evaluated once at materialize time (tick 1): `every 1` fires its body,
    # `every 2+` skips it. The skip case materializes to zero nodes, which
    # must resolve as a no-op success, not trip the deadlock guard.
    @every_one_sym ~s|workflow "tick" on manual { every 1 of gc { b <- agent { engine: codex, model: "m", prompt: inline "act" } } }|
    @every_two_sym ~s|workflow "tick" on manual { every 2 of gc { b <- agent { engine: codex, model: "m", prompt: inline "act" } } }|

    test "every 1 fires its body on the first tick of a supervised run", %{dir: dir} do
      g = materialized("run-every-fire", @every_one_sym)

      # The body fires immediately at materialize (tick 1), so the body
      # agent is present from the start with no placeholder to resolve.
      assert Map.has_key?(g.nodes, "agent-1")

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-every-fire", dir: dir)
      assert final.status == :succeeded
      assert final.nodes["agent-1"].state == :succeeded
    end

    test "every 2 skips on the first tick and the no-op run succeeds without deadlock", %{dir: dir} do
      g = materialized("run-every-skip", @every_two_sym)

      # The gate does not fire on tick 1, so nothing materializes. A run with
      # no schedulable work is a no-op success, not a deadlock.
      assert g.nodes == %{}

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-every-skip", dir: dir)
      assert final.status == :succeeded
      assert final.nodes == %{}
    end
  end

  describe "map fan-out execution (Phase 8)" do
    # A seed agent whose output is a list, then a `map ${seed.repos} as repo`
    # body that fans out one child agent per element. The interpreter ids are
    # content-derived: the seed is `agent-0`, the unresolved fan-out is the
    # `map-1` placeholder, and each child is `agent-2-<digest>` keyed on the
    # element index. The supervised run drives `agent-0` to success, re-expands
    # on its list output, then schedules every child.
    @map_sym ~s|workflow "fan" on manual { seed <- agent { engine: codex, model: "m", prompt: inline "list" } map ${seed.repos} as repo { child <- agent { engine: codex, model: "m", prompt: inline "audit ${repo}" } } }|

    test "a map over a dependency's list fans out one child per element and collects every output", %{dir: dir} do
      g = materialized("run-map-fanout", @map_sym)

      # Before the seed succeeds the body is a single placeholder, not work.
      assert g.nodes["map-1"].kind == :map_fanout
      refute Enum.any?(Map.values(g.nodes), &(&1.kind == :agent and &1.id != "agent-0"))

      # The seed yields three repos; each child is unprogrammed and falls
      # through the fake's default success, so the run drives all three to
      # :succeeded without per-child programming.
      FakeEngine.program("agent-0", {:ok, %{repos: ["alpha", "beta", "gamma"]}})

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-map-fanout", dir: dir)
      assert final.status == :succeeded
      assert final.nodes["agent-0"].state == :succeeded

      # One child node per element, each terminal succeeded. The ids are the
      # content-derived fan-out keys; assert by count and kind rather than
      # spelling each digest, so a stable-id change does not break the test.
      children = for {_id, %Node{ast_origin: "agent-2"} = n} <- final.nodes, do: n
      assert length(children) == 3
      assert Enum.all?(children, &(&1.state == :succeeded))

      # Every child's output collects back into the graph (here the fake's
      # default `%{default: id}`), so a downstream node could read any one.
      assert Enum.all?(children, fn n -> n.output == %{default: n.id} end)

      # The resolved fan-out placeholder is retired to :skipped, the
      # load-bearing pair with the deadlock guard: a fanned-out placeholder
      # must not sit :pending and stall the run.
      assert final.nodes["map-1"].state == :skipped
    end

    # A map over an empty list emits zero children. The placeholder retires to
    # :skipped, leaving only the succeeded seed, so the run completes as a
    # no-op success rather than tripping the deadlock guard.
    test "a map over an empty list emits no children and the run still succeeds", %{dir: dir} do
      g = materialized("run-map-empty", @map_sym)

      FakeEngine.program("agent-0", {:ok, %{repos: []}})

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-map-empty", dir: dir)
      assert final.status == :succeeded
      assert final.nodes["agent-0"].state == :succeeded
      # No child node was emitted for an empty fan-out.
      refute Enum.any?(Map.values(final.nodes), &(&1.ast_origin == "agent-2"))
      # The placeholder is retired so the empty fan-out does not stall the run.
      assert final.nodes["map-1"].state == :skipped
    end
  end

  describe "run visibility at creation" do
    test "a freshly started run is present in the store before any node finishes", %{dir: dir} do
      run_id = "run-visible-at-creation"

      # Use a slow node so the run is in-flight when we check the store.
      g = materialized(run_id, ~s|workflow "vis" on manual { a <- agent { engine: codex, model: "m", prompt: inline "x" } }|)
      # The first scheduling pass will call run_attempt; sleep so we can load
      # from the store before the fake engine returns.
      FakeEngine.program("agent-0", {:sleep_then, {:ok, :done}})

      {:ok, _pid} = Runtime.start_link(g, opts(dir))

      # Load the store immediately after start_link returns. The run must be
      # present on disk because init/1 persists before the first scheduling
      # pass, even while a slow placement acquire (or in this test, a sleeping
      # fake engine) is still in flight.
      assert {:ok, visible} = Store.load(run_id, dir: dir)
      assert visible.status == :running
      assert map_size(visible.nodes) == 1
    end
  end

  describe "operator hooks" do
    test "cancel stops the run and marks non-terminal nodes cancelled", %{dir: dir} do
      g =
        graph("run-cancel", [
          node("a", state: :pending, inputs: %{"x" => {:node, "slow", []}}),
          node("slow", state: :pending)
        ])

      # `slow` sleeps so the run is still in flight when we cancel.
      FakeEngine.program("slow", {:sleep_then, {:ok, :late}})

      {:ok, pid} = Runtime.start_link(g, opts(dir))
      :ok = Runtime.cancel(pid)
      wait_for_exit(pid)

      {:ok, final} = Store.load("run-cancel", dir: dir)
      assert final.status == :cancelled
      assert final.nodes["a"].state == :cancelled
    end
  end
end
