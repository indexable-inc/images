defmodule IxMcp.Agents.MetricsTest do
  use ExUnit.Case, async: true

  alias IxMcp.Agents.Metrics

  @moduletag :os_procs

  defp beam_os_pid, do: String.to_integer(System.pid())

  test "sampling this BEAM reports what the platform grants and names what it does not" do
    os_pid = beam_os_pid()

    assert %{^os_pid => proc} = Metrics.sample([os_pid])
    assert proc.os_pid == os_pid
    assert proc.command != ""
    assert is_float(proc.cpu_pct)

    case :os.type() do
      # macOS 27 refuses rss and IO to an unentitled reader; a fabricated zero
      # would be indistinguishable from an idle process (see the moduledoc).
      {:unix, :darwin} ->
        assert proc.rss_kb == :unavailable
        assert proc.io == :unavailable

      {:unix, :linux} ->
        assert proc.rss_kb > 0
        assert %{read_bytes: _read, write_bytes: _write} = proc.io
    end
  end

  test "a pid that exited between the walk and the sample is absent, not an error" do
    # The real turnover shape: a pid that was alive moments ago. Reusing a
    # freshly dead one rather than a made-up large number, because ps treats
    # those differently -- an out-of-range pid makes it refuse the whole request
    # and return nothing, which would have made this test pass for the wrong
    # reason.
    {out, 0} = IxMcp.Cmd.run("sh", ["-c", "echo $$"])
    dead = out |> String.trim() |> String.to_integer()

    sampled = Metrics.sample([beam_os_pid(), dead])

    assert Map.keys(sampled) == [beam_os_pid()]
  end

  test "sampling nothing costs nothing" do
    assert Metrics.sample([]) == %{}
  end

  test "totals add up, or say they cannot" do
    for agent <- Metrics.tree() do
      assert is_binary(agent.id)

      case agent.rss_kb do
        :unavailable -> assert Enum.any?(agent.procs, &(&1.rss_kb == :unavailable))
        total -> assert total == Enum.sum(Enum.map(agent.procs, & &1.rss_kb))
      end
    end
  end
end
