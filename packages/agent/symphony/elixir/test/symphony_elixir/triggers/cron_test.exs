defmodule SymphonyElixir.Triggers.CronTest do
  # The loud-skip test boots the shared named singletons (WorkflowCatalog,
  # CronState, Triggers.Cron), so this file cannot run concurrently with
  # other tests that start them.
  use ExUnit.Case, async: false

  import ExUnit.CaptureLog

  alias SymphonyElixir.CronExpression
  alias SymphonyElixir.CronState
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.Triggers.Cron
  alias SymphonyElixir.WorkflowCatalog

  # US DST in 2026: spring forward Sun Mar 8 (02:00 PST -> 03:00 PDT),
  # fall back Sun Nov 1 (02:00 PDT -> 01:00 PST). PST=UTC-8, PDT=UTC-7.
  @la "America/Los_Angeles"

  defp parsed!(schedule) do
    {:ok, parsed} = CronExpression.parse(schedule)
    parsed
  end

  describe "next_due/4 plain zone offset" do
    test "9am LA is 17:00 UTC in winter (PST)" do
      last = ~U[2026-01-15 17:30:00Z]
      now = ~U[2026-01-16 17:00:30Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("0 9 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-01-16 17:00:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-01-16T09:00:00-08:00"
    end

    test "9am LA is 16:00 UTC in summer (PDT)" do
      last = ~U[2026-07-15 16:30:00Z]
      now = ~U[2026-07-16 16:00:30Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("0 9 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-07-16 16:00:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-07-16T09:00:00-07:00"
    end

    test "not due before the local moment arrives" do
      last = ~U[2026-07-15 16:30:00Z]
      now = ~U[2026-07-16 15:59:00Z]

      assert Cron.next_due(parsed!("0 9 * * *"), last, now, @la) == :not_due
    end

    test "the UTC default zone behaves like plain UTC cron" do
      last = ~U[2026-05-17 08:31:00Z]
      now = ~U[2026-05-17 09:00:10Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("0 9 * * *"), last, now, "UTC")
      assert DateTime.compare(scheduled_for, ~U[2026-05-17 09:00:00Z]) == :eq
    end
  end

  describe "next_due/4 spring-forward gap" do
    test "a 02:30 that does not exist fires at the first valid instant after the gap" do
      # Fired Mar 7 at 02:30 PST; the watermark sits just after it.
      last = ~U[2026-03-07 10:31:00Z]
      # 03:00 PDT (the gap's end) is 10:00 UTC.
      now = ~U[2026-03-08 10:00:30Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("30 2 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-03-08 10:00:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-03-08T03:00:00-07:00"
    end

    test "the gap fire is not due before the transition moment" do
      last = ~U[2026-03-07 10:31:00Z]
      now = ~U[2026-03-08 09:59:00Z]

      assert Cron.next_due(parsed!("30 2 * * *"), last, now, @la) == :not_due
    end
  end

  describe "next_due/4 fall-back repeat" do
    test "a repeated 01:30 fires at the first (earlier) occurrence" do
      last = ~U[2026-10-31 08:31:00Z]
      # First 01:30 is PDT = 08:30 UTC; the repeat (PST) is 09:30 UTC.
      now = ~U[2026-11-01 08:31:00Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("30 1 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-11-01 08:30:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-11-01T01:30:00-07:00"
    end

    test "the watermark from the first occurrence swallows the repeat: one fire, not two" do
      # The fire above records last_fired_at = now (01:31 PDT). When the
      # wall clock reads 01:30 again an hour later (PST), the next
      # wall-clock match after the watermark is tomorrow's 01:30.
      watermark = ~U[2026-11-01 08:31:00Z]
      second_occurrence = ~U[2026-11-01 09:30:30Z]

      assert Cron.next_due(parsed!("30 1 * * *"), watermark, second_occurrence, @la) == :not_due
    end
  end

  describe "next_due/4 unknown zone" do
    test "an unknown zone is an error, never a silent UTC fallback" do
      last = ~U[2026-01-15 17:30:00Z]
      now = ~U[2026-01-16 17:00:30Z]

      assert Cron.next_due(parsed!("0 9 * * *"), last, now, "Not/AZone") ==
               {:error, :time_zone_not_found}
    end
  end

  describe "tick with an unknown zone" do
    @tag :tmp_dir
    test "logs a warning and keeps ticking instead of crashing", %{tmp_dir: dir} do
      File.write!(
        Path.join(dir, "badtz.sym"),
        ~s|workflow "badtz" on cron "* * * * *" tz "Not/AZone" { a <- exec "./x.sh" }|
      )

      start_supervised!({WorkflowCatalog, workflows_dir: dir, poll_ms: 60_000})
      start_supervised!(CronState)
      cron = start_supervised!({Cron, []})

      # start_supervised!/1 already ran the catalog's synchronous boot scan,
      # so this just re-scans; kept explicit so the workflow's presence
      # before the first poll below isn't an implicit assumption.
      :ok = WorkflowCatalog.scan(dir)

      # First poll seeds the watermark; the second evaluates against it and
      # hits the zone lookup.
      log =
        capture_log(fn ->
          :ok = Cron.poll_now()
          :ok = Cron.poll_now()
        end)

      assert log =~ "Cron timezone unknown for workflow=badtz"
      assert log =~ "Not/AZone"
      assert Process.alive?(cron)
    end
  end

  describe "tick firing (full stack, fake engine)" do
    defmodule FakeEngine do
      @moduledoc false
      @behaviour SymphonyElixir.Runtime.EngineClient

      @impl true
      def run_node(%Node{id: id}, _opts), do: {:ok, %{ran: id}, "thread-#{id}"}

      @impl true
      def status(_thread_id), do: :unknown
    end

    setup %{tmp_dir: base} do
      start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
      start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})
      start_supervised!(SymphonyElixir.Runtime.Supervisor)
      start_supervised!(CronState)

      store = Path.join(base, "store")
      workflows = Path.join(base, "workflows")
      File.mkdir_p!(store)
      File.mkdir_p!(workflows)
      start_supervised!({WorkflowCatalog, workflows_dir: workflows, poll_ms: 60_000})

      store_opts = [dir: store]
      start_supervised!({Cron, run_opts: [engine: FakeEngine, store_opts: store_opts]})

      {:ok, store_opts: store_opts, workflows_dir: workflows}
    end

    # Workflow names are unique per test because CronState persists to the
    # shared test-root cron_state.json across this file's tests.
    defp unique_name(prefix), do: "#{prefix}-#{System.unique_integer([:positive])}"

    defp write_cron_sym!(dir, name, header_rest) do
      source = ~s|workflow "#{name}" on cron #{header_rest} { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|
      File.write!(Path.join(dir, "#{name}.sym"), source)
      WorkflowCatalog.scan(dir)
    end

    # Only stable snapshots: the store writes temp-then-rename, so a raw
    # File.ls! can catch a transient <run_id>.json.tmp mid-persist.
    defp run_files(store_opts) do
      store_opts
      |> Keyword.fetch!(:dir)
      |> Path.join("*.json")
      |> Path.wildcard()
      |> Enum.map(&Path.basename/1)
    end

    # Same tolerance as IngressTest: start_link returns before the :advance
    # continuation writes the first snapshot.
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

    defp wait_run_count(store_opts, expected, attempts \\ 60) do
      files = run_files(store_opts)

      cond do
        length(files) == expected ->
          files

        attempts == 0 ->
          flunk("expected #{expected} run files, have #{inspect(files)}")

        true ->
          Process.sleep(20)
          wait_run_count(store_opts, expected, attempts - 1)
      end
    end

    @tag :tmp_dir
    test "first observation seeds the watermark without firing", %{store_opts: store_opts, workflows_dir: dir} do
      name = unique_name("seed")
      write_cron_sym!(dir, name, ~s|"* * * * *"|)

      assert :ok = Cron.poll_now()

      assert %DateTime{} = CronState.get_last_fired(name)
      assert run_files(store_opts) == []
    end

    @tag :tmp_dir
    test "fires exactly one zoned catch-up run and stamps the cron trigger", %{store_opts: store_opts, workflows_dir: dir} do
      name = unique_name("daily")
      write_cron_sym!(dir, name, ~s|"0 9 * * *" tz "America/Los_Angeles"|)

      # A watermark two days back has a passed 9am-LA match, so the next
      # tick owes exactly one catch-up fire.
      :ok = CronState.seed_if_unset(name, DateTime.add(DateTime.utc_now(), -2, :day))

      assert :ok = Cron.poll_now()
      [run_file] = wait_run_count(store_opts, 1)

      # Let the run finish before the second tick so no snapshot persist
      # races the file-count assertions below.
      run_id = Path.rootname(run_file)
      graph = wait_terminal(run_id, store_opts)
      assert %{kind: :cron, schedule: "0 9 * * *", timezone: "America/Los_Angeles"} = graph.trigger

      # The fire advanced the watermark to now, so an immediate second tick
      # owes nothing: the watermark is untouched (record_fire is the only
      # writer) and no second run appears.
      watermark = CronState.get_last_fired(name)
      assert :ok = Cron.poll_now()
      assert CronState.get_last_fired(name) == watermark
      assert run_files(store_opts) == [run_file]
    end

    @tag :tmp_dir
    test "an unparseable schedule is logged and skipped without poisoning the tick", %{workflows_dir: dir} do
      bad = unique_name("bad")
      good = unique_name("good")
      write_cron_sym!(dir, bad, ~s|"not a cron"|)
      write_cron_sym!(dir, good, ~s|"* * * * *"|)

      assert :ok = Cron.poll_now()

      # The bad schedule cannot seed (it never parses), the good one still
      # did: the tick walked past the failure instead of crashing.
      assert is_nil(CronState.get_last_fired(bad))
      assert %DateTime{} = CronState.get_last_fired(good)
    end
  end
end
