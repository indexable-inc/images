defmodule SymphonyElixir.CommandTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Command

  test "captures successful command output" do
    assert {:ok, "ok\n"} = Command.run("/bin/sh", ["-c", "printf 'ok\n'"], 1_000)
  end

  test "captures stderr with failed command output" do
    assert {:error, {:exit, 7, "bad\n"}} = Command.run("/bin/sh", ["-c", "printf 'bad\n' >&2; exit 7"], 1_000)
  end

  test "terminates commands after the timeout" do
    assert {:error, {:timeout, 50, _output}} = Command.run("/bin/sh", ["-c", "sleep 5"], 50)
  end

  test "kills the spawned process on timeout so it does not orphan" do
    pid_file = Path.join(System.tmp_dir!(), "command_test_#{System.unique_integer([:positive])}.pid")
    on_exit(fn -> File.rm(pid_file) end)

    # `exec sleep` replaces the shell so $$ is the surviving process the
    # port owns; without the kill it would outlive the 50ms timeout.
    assert {:error, {:timeout, 50, _output}} =
             Command.run("/bin/sh", ["-c", "echo $$ > #{pid_file}; exec sleep 30"], 50)

    os_pid = wait_for_pid(pid_file)
    assert eventually_dead?(os_pid), "spawned process #{os_pid} was left running after timeout"
  end

  defp wait_for_pid(pid_file, attempts \\ 50) do
    case File.read(pid_file) do
      {:ok, contents} when contents != "" -> contents |> String.trim() |> String.to_integer()
      _ when attempts > 0 -> Process.sleep(10) && wait_for_pid(pid_file, attempts - 1)
      _ -> flunk("spawned process never recorded its pid in #{pid_file}")
    end
  end

  defp eventually_dead?(os_pid, attempts \\ 50)

  defp eventually_dead?(_os_pid, 0), do: false

  defp eventually_dead?(os_pid, attempts) do
    case System.cmd("kill", ["-0", Integer.to_string(os_pid)], stderr_to_stdout: true) do
      {_, 0} -> Process.sleep(10) && eventually_dead?(os_pid, attempts - 1)
      {_, _nonzero} -> true
    end
  end
end
