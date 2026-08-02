defmodule Loom.GuardTest do
  use ExUnit.Case, async: false

  test "fork? flips exactly when the identity changes" do
    dir = Path.join(System.tmp_dir!(), "loom-guard-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    id_file = Path.join(dir, "identity")
    File.write!(id_file, "identity-A\n")

    previous = Application.get_env(:loom, :identity_cmd)
    Application.put_env(:loom, :identity_cmd, {"cat", [id_file]})

    on_exit(fn ->
      case previous do
        nil -> Application.delete_env(:loom, :identity_cmd)
        value -> Application.put_env(:loom, :identity_cmd, value)
      end

      # Re-baseline the shared Guard on the restored probe so later
      # tests never see this test's identity file.
      Agent.stop(Loom.Guard)
      await_guard()
      File.rm_rf!(dir)
    end)

    # Restart the supervised Guard so it baselines on the file probe.
    Agent.stop(Loom.Guard)
    await_guard()

    assert Loom.Guard.baseline() == "identity-A"
    refute Loom.Guard.fork?()

    # The fork: same process, different network identity.
    File.write!(id_file, "identity-B\n")
    assert Loom.Guard.fork?()

    # And back: the original never trips.
    File.write!(id_file, "identity-A\n")
    refute Loom.Guard.fork?()
  end

  defp await_guard(deadline_ms \\ 2_000) do
    deadline = System.monotonic_time(:millisecond) + deadline_ms

    Stream.repeatedly(fn ->
      Process.sleep(10)
      Process.whereis(Loom.Guard)
    end)
    |> Enum.find(fn pid ->
      is_pid(pid) or System.monotonic_time(:millisecond) > deadline
    end)
    |> is_pid()
    |> case do
      true -> :ok
      false -> flunk("Loom.Guard did not restart")
    end
  end
end
