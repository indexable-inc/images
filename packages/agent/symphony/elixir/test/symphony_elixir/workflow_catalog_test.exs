defmodule SymphonyElixir.WorkflowCatalogTest do
  use ExUnit.Case, async: false

  alias SymphonyElixir.WorkflowCatalog

  @moduletag capture_log: true

  setup do
    dir = Path.join(System.tmp_dir!(), "wf_catalog_#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    on_exit(fn -> File.rm_rf(dir) end)
    # A long poll so the only scans are the boot scan and the explicit ones
    # the test drives; keeps assertions deterministic.
    start_supervised!({WorkflowCatalog, workflows_dir: dir, poll_ms: 60_000})
    {:ok, dir: dir}
  end

  defp write_sym!(dir, name, body) do
    File.write!(Path.join(dir, "#{name}.sym"), body)
  end

  test "parses .sym files and indexes them by name and trigger", %{dir: dir} do
    write_sym!(dir, "implement", ~s|workflow "implement" on linear label "[sym] implement" { a <- agent { engine: codex, model: "m", prompt: skill "implement" {} } }|)
    write_sym!(dir, "nightly", ~s|workflow "nightly" on cron "0 9 * * *" tz "UTC" { gc <- exec "./gc.sh" }|)

    WorkflowCatalog.scan(dir)

    assert {:ok, impl} = WorkflowCatalog.workflow("implement")
    assert impl.name == "implement"
    assert impl.trigger == %{kind: :linear, label: "[sym] implement"}
    assert is_binary(impl.hash)

    assert WorkflowCatalog.workflows() |> Enum.map(& &1.name) |> Enum.sort() == ["implement", "nightly"]
    assert [%{name: "implement"}] = WorkflowCatalog.for_trigger_kind(:linear)
    assert [%{name: "nightly"}] = WorkflowCatalog.for_trigger_kind(:cron)
  end

  test "hot-reloads changed bytes and drops deleted files", %{dir: dir} do
    write_sym!(dir, "w", ~s|workflow "w" on manual { a <- exec "./x.sh" }|)
    WorkflowCatalog.scan(dir)
    assert {:ok, %{hash: first}} = WorkflowCatalog.workflow("w")

    write_sym!(dir, "w", ~s|workflow "w" on cron "* * * * *" { a <- exec "./x.sh" }|)
    WorkflowCatalog.scan(dir)
    assert {:ok, reloaded} = WorkflowCatalog.workflow("w")
    assert reloaded.hash != first
    assert reloaded.trigger.kind == :cron

    File.rm!(Path.join(dir, "w.sym"))
    WorkflowCatalog.scan(dir)
    assert WorkflowCatalog.workflow("w") == {:error, :not_found}
  end

  test "a parse error keeps the last good version in place", %{dir: dir} do
    write_sym!(dir, "w", ~s|workflow "w" on manual { a <- exec "./x.sh" }|)
    WorkflowCatalog.scan(dir)
    assert {:ok, good} = WorkflowCatalog.workflow("w")

    write_sym!(dir, "w", ~s|workflow "w" on manual { this is not valid |)
    WorkflowCatalog.scan(dir)
    # The broken bytes are rejected; the prior parse stays published.
    assert {:ok, ^good} = WorkflowCatalog.workflow("w")
  end

  test "a parse error is recorded with a located, file-stamped diagnostic", %{dir: dir} do
    write_sym!(dir, "w", ~s|workflow "w" on manual { a <- exec "./x.sh" }|)
    write_sym!(dir, "broken", "workflow \"broken\" {\n  oops\n}\n")
    WorkflowCatalog.scan(dir)

    # The good file parses; the broken one is absent from the published set
    # but present in the error set.
    assert {:ok, _} = WorkflowCatalog.workflow("w")
    assert WorkflowCatalog.workflow("broken") == {:error, :not_found}

    assert {:ok, err} = WorkflowCatalog.error("broken")
    assert err.name == "broken"
    assert err.file == "broken.sym"
    assert is_integer(err.line) and err.line >= 1
    assert is_integer(err.column) and err.column >= 1
    assert is_binary(err.message)

    # The healthy file has no recorded error, and `errors/0` lists only the
    # broken one.
    assert WorkflowCatalog.error("w") == {:error, :not_found}
    assert Enum.map(WorkflowCatalog.errors(), & &1.name) == ["broken"]
  end

  test "a recorded error clears when the file parses again", %{dir: dir} do
    write_sym!(dir, "w", "workflow \"w\" {\n  oops\n}\n")
    WorkflowCatalog.scan(dir)
    assert {:ok, _} = WorkflowCatalog.error("w")

    write_sym!(dir, "w", ~s|workflow "w" on manual { a <- exec "./x.sh" }|)
    WorkflowCatalog.scan(dir)
    assert WorkflowCatalog.error("w") == {:error, :not_found}
    assert {:ok, _} = WorkflowCatalog.workflow("w")
  end

  test "deleting a broken file retires its recorded error", %{dir: dir} do
    write_sym!(dir, "broken", "workflow \"broken\" {\n  oops\n}\n")
    WorkflowCatalog.scan(dir)
    assert {:ok, _} = WorkflowCatalog.error("broken")

    File.rm!(Path.join(dir, "broken.sym"))
    WorkflowCatalog.scan(dir)
    assert WorkflowCatalog.error("broken") == {:error, :not_found}
    assert WorkflowCatalog.errors() == []
  end
end
