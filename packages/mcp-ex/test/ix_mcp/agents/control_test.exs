defmodule IxMcp.Agents.ControlTest do
  # async: false: the registry is the application's, shared with any runner a
  # sibling test spawns.
  use ExUnit.Case, async: false

  import IxMcpTest.Eventually

  alias IxMcp.Agents.Control

  defp entry(pid), do: %{runner: pid, os_pid: nil, stdin: :stream, backend: :claude}

  defp register_from(id, entry) do
    test = self()

    pid =
      spawn(fn ->
        :ok = Control.register(id, %{entry | runner: self()})
        send(test, :registered)

        receive do
          :stop -> :ok
        end
      end)

    assert_receive :registered, 1_000
    pid
  end

  test "an entry lives as long as the runner that registered it" do
    pid = register_from("ctl-live", entry(nil))

    assert {:ok, %{runner: ^pid, stdin: :stream}} = Control.lookup("ctl-live")
    assert %{"ctl-live" => %{runner: ^pid}} = Map.take(Control.all(), ["ctl-live"])

    send(pid, :stop)

    # Registry sweeps on a monitor message, so the entry goes on its own.
    eventually(fn -> if Control.lookup("ctl-live") == :error, do: true end)
    refute Map.has_key?(Control.all(), "ctl-live")
  end

  test "a phase turning over never collides with the one it replaces" do
    # The reason the keys are duplicate: a woken child registers its new runner
    # while the previous phase's entry may not be swept yet. With unique keys
    # this second register would error; here the live one is the answer.
    old = register_from("ctl-turnover", entry(nil))
    Process.exit(old, :kill)
    new = register_from("ctl-turnover", entry(nil))

    assert {:ok, %{runner: ^new}} = Control.lookup("ctl-turnover")
    send(new, :stop)
  end

  test "registering for another process is a caller error, not a silent entry" do
    other = spawn(fn -> Process.sleep(:infinity) end)

    assert_raise FunctionClauseError, fn -> Control.register("ctl-alien", entry(other)) end

    Process.exit(other, :kill)
  end
end
