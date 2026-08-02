defmodule Loom.FakeIx do
  @moduledoc """
  Test harness around `test/support/fake-ix`: points `Loom.Ix` at the
  recording fake, owns the per-test scratch dir, and reads back the
  recorded argv lines so tests assert exact verb sequences and counts
  (a green run that recorded zero verbs is a broken test, not a pass).
  """

  @spec setup(map()) :: keyword()
  def setup(context) do
    dir =
      Path.join(
        System.tmp_dir!(),
        "loom-fake-ix-#{System.unique_integer([:positive])}"
      )

    File.mkdir_p!(dir)
    log = Path.join(dir, "calls.log")
    File.write!(log, "")

    script = Path.expand("fake-ix", __DIR__)
    File.chmod!(script, 0o755)

    previous = Application.get_env(:loom, :ix_bin)
    Application.put_env(:loom, :ix_bin, script)
    System.put_env("LOOM_FAKE_IX_LOG", log)
    System.put_env("LOOM_FAKE_SNAPSHOT_ID", snapshot_id())

    ExUnit.Callbacks.on_exit(fn ->
      restore_env(previous)
      File.rm_rf!(dir)
    end)

    stream = Path.join(dir, "stream.ndjson")
    write_stream(stream, default_stream())
    System.put_env("LOOM_FAKE_STREAM_FILE", stream)

    Map.to_list(context) ++ [fake_dir: dir, calls_log: log, stream_file: stream]
  end

  @spec snapshot_id() :: String.t()
  def snapshot_id, do: "11111111-2222-3333-4444-555555555555"

  @doc "Recorded invocations, each as the argv list of one `ix` call."
  @spec calls(String.t()) :: [[String.t()]]
  def calls(log) do
    log
    |> File.read!()
    |> String.split("\n", trim: true)
    |> Enum.map(&String.split(&1, "\t"))
  end

  @doc "Block until `n` calls are recorded or `timeout_ms` elapses."
  @spec await_calls(String.t(), pos_integer(), non_neg_integer()) :: [[String.t()]]
  def await_calls(log, n, timeout_ms \\ 5_000) do
    deadline = System.monotonic_time(:millisecond) + timeout_ms
    poll_calls(log, n, deadline)
  end

  @spec write_stream(String.t(), [map()]) :: :ok
  def write_stream(path, events) do
    File.write!(path, Enum.map_join(events, "", fn e -> JSON.encode!(e) <> "\n" end))
  end

  @spec default_stream() :: [map()]
  def default_stream do
    [
      %{"type" => "system", "subtype" => "init", "session_id" => "sess-fixture-1"},
      %{"type" => "assistant", "message" => %{"content" => "working"}},
      %{"type" => "result", "subtype" => "success", "result" => "the final answer"}
    ]
  end

  @spec poll_calls(String.t(), pos_integer(), integer()) :: [[String.t()]]
  defp poll_calls(log, n, deadline) do
    recorded = calls(log)

    cond do
      length(recorded) >= n ->
        recorded

      System.monotonic_time(:millisecond) > deadline ->
        recorded

      true ->
        Process.sleep(20)
        poll_calls(log, n, deadline)
    end
  end

  @spec restore_env(term()) :: :ok
  defp restore_env(previous) do
    case previous do
      nil -> Application.delete_env(:loom, :ix_bin)
      bin -> Application.put_env(:loom, :ix_bin, bin)
    end

    Enum.each(
      ["LOOM_FAKE_IX_LOG", "LOOM_FAKE_SNAPSHOT_ID", "LOOM_FAKE_STREAM_FILE", "LOOM_FAKE_FAIL"],
      &System.delete_env/1
    )
  end
end
