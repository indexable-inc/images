defmodule SymphonyElixir.Runtime.IngressTest do
  use ExUnit.Case, async: false

  alias SymphonyElixir.DSL.Parser
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.Runtime.Ingress
  alias SymphonyElixir.WorkflowCatalog

  @moduletag capture_log: true

  defmodule FakeEngine do
    @moduledoc false
    @behaviour SymphonyElixir.Runtime.EngineClient

    @impl true
    def run_node(%Node{id: id}, _opts), do: {:ok, %{ran: id}, "thread-#{id}"}

    @impl true
    def status(_thread_id), do: :unknown
  end

  setup do
    start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})
    start_supervised!(SymphonyElixir.Runtime.Supervisor)

    tmp = Path.join(System.tmp_dir!(), "ingress_#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)
    on_exit(fn -> File.rm_rf(tmp) end)

    # A catalog over an isolated workflows dir so `start_by_trigger/2`
    # resolves against the .sym files this test wrote, not the bundled pack.
    workflows_dir = Path.join(System.tmp_dir!(), "ingress_wf_#{System.unique_integer([:positive])}")
    File.mkdir_p!(workflows_dir)
    on_exit(fn -> File.rm_rf(workflows_dir) end)
    start_supervised!({WorkflowCatalog, workflows_dir: workflows_dir, poll_ms: 60_000})

    {:ok, store_opts: [dir: tmp], workflows_dir: workflows_dir}
  end

  defp write_sym!(dir, name, body) do
    File.write!(Path.join(dir, "#{name}.sym"), body)
  end

  defp entry(source) do
    {:ok, ast} = Parser.parse(source)
    %{name: ast.name, ast: ast, trigger: ast.trigger, source: source, hash: :crypto.hash(:sha256, source)}
  end

  # Tolerate the not-yet-persisted window: start_link returns before the
  # :advance continuation writes the first snapshot, so the run file may be
  # absent on the first poll.
  defp wait_terminal(run_id, store_opts, attempts \\ 60) do
    case Store.load(run_id, store_opts) do
      {:ok, %{status: status} = graph} when status in [:succeeded, :failed, :cancelled] ->
        graph

      _ when attempts == 0 ->
        flunk("run #{run_id} never terminal")

      _ ->
        Process.sleep(20)
        wait_terminal(run_id, store_opts, attempts - 1)
    end
  end

  test "materializes a workflow and runs it under supervision", %{store_opts: store_opts} do
    e = entry(~s|workflow "demo" on manual { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)

    assert {:ok, %{run_id: run_id, pid: pid}} =
             Ingress.start_workflow(e, %{kind: :manual, input: %{}}, engine: FakeEngine, store_opts: store_opts)

    assert is_pid(pid)
    final = wait_terminal(run_id, store_opts)

    assert final.status == :succeeded
    assert final.source_hash == e.hash
    # The trigger event is stamped on the run and survives the store round-trip.
    assert final.trigger == %{kind: :manual, input: %{}}
  end

  test "the generated run id is slugged from the workflow name", %{store_opts: store_opts} do
    e = entry(~s|workflow "Nightly GC" on manual { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)

    assert {:ok, %{run_id: run_id}} =
             Ingress.start_workflow(e, nil, engine: FakeEngine, store_opts: store_opts)

    assert String.starts_with?(run_id, "nightly-gc-")
  end

  test "an explicit run_id is honored", %{store_opts: store_opts} do
    e = entry(~s|workflow "w" on manual { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)

    assert {:ok, %{run_id: "fixed-id"}} =
             Ingress.start_workflow(e, nil, run_id: "fixed-id", engine: FakeEngine, store_opts: store_opts)

    assert wait_terminal("fixed-id", store_opts).status == :succeeded
  end

  test "start_by_trigger fans out to every workflow matching the event", %{store_opts: store_opts, workflows_dir: dir} do
    write_sym!(dir, "label-a", ~s|workflow "label-a" on github_pr_label repo "acme/app" label "ship" { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)
    write_sym!(dir, "label-b", ~s|workflow "label-b" on github_pr_label repo "acme/app" label "ship" { b <- agent { engine: codex, model: "m", prompt: inline "go" } }|)
    write_sym!(dir, "other-repo", ~s|workflow "other-repo" on github_pr_label repo "acme/other" label "ship" { c <- agent { engine: codex, model: "m", prompt: inline "go" } }|)
    WorkflowCatalog.scan(dir)

    event = %{kind: :github_pr_label, repo: "acme/app", label: "ship", pr_number: 7}

    assert {:ok, started} = Ingress.start_by_trigger(event, engine: FakeEngine, store_opts: store_opts)
    assert length(started) == 2

    for %{run_id: run_id} <- started do
      final = wait_terminal(run_id, store_opts)
      assert final.status == :succeeded
      # The inbound event is the run's trigger context.
      assert final.trigger == event
    end
  end

  test "start_by_trigger is a no-op when no workflow matches", %{store_opts: store_opts, workflows_dir: dir} do
    write_sym!(dir, "label-a", ~s|workflow "label-a" on github_pr_label repo "acme/app" label "ship" { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)
    WorkflowCatalog.scan(dir)

    assert {:ok, []} =
             Ingress.start_by_trigger(
               %{kind: :github_pr_label, repo: "acme/app", label: "nope"},
               engine: FakeEngine,
               store_opts: store_opts
             )
  end

  test "start_by_trigger matches a linear label against the event's labels", %{store_opts: store_opts, workflows_dir: dir} do
    write_sym!(dir, "triage", ~s|workflow "triage" on linear label "[sym] triage" { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)
    WorkflowCatalog.scan(dir)

    event = %{kind: :linear, labels: ["other", "[sym] triage"], issue_id: "ISS-1"}

    assert {:ok, [%{run_id: run_id}]} =
             Ingress.start_by_trigger(event, engine: FakeEngine, store_opts: store_opts)

    assert wait_terminal(run_id, store_opts).status == :succeeded
  end

  test "seen_trigger? is the producer dedup read over IR runs", %{store_opts: store_opts, workflows_dir: dir} do
    write_sym!(dir, "triage", ~s|workflow "triage" on linear label "[sym] triage" { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)
    WorkflowCatalog.scan(dir)

    issue_match = fn
      {_status, %{kind: :linear, issue_id: "ISS-7"}} -> true
      {_status, _trigger} -> false
    end

    refute Ingress.seen_trigger?(issue_match, store_opts: store_opts)

    event = %{kind: :linear, labels: ["[sym] triage"], issue_id: "ISS-7"}
    assert {:ok, [%{run_id: run_id}]} = Ingress.start_by_trigger(event, engine: FakeEngine, store_opts: store_opts)
    wait_terminal(run_id, store_opts)

    # The run persisted its trigger, so the dedup read now sees the issue.
    assert Ingress.seen_trigger?(issue_match, store_opts: store_opts)
  end
end
