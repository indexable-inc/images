defmodule SymphonyElixir.Command do
  @moduledoc false

  @type result :: {:ok, String.t()} | {:error, {:exit, non_neg_integer(), String.t()} | {:timeout, pos_integer(), String.t()} | {:start_failed, String.t()}}

  @spec run(Path.t(), [String.t()], pos_integer()) :: result()
  def run(executable, args, timeout_ms) when is_binary(executable) and is_list(args) and timeout_ms > 0 do
    port =
      Port.open({:spawn_executable, executable}, [
        :binary,
        :exit_status,
        :stderr_to_stdout,
        args: args
      ])

    deadline = System.monotonic_time(:millisecond) + timeout_ms
    collect(port, deadline, timeout_ms, [])
  rescue
    error -> {:error, {:start_failed, Exception.message(error)}}
  end

  defp collect(port, deadline, timeout_ms, chunks) do
    remaining_ms = max(deadline - System.monotonic_time(:millisecond), 0)

    receive do
      {^port, {:data, data}} ->
        collect(port, deadline, timeout_ms, [data | chunks])

      {^port, {:exit_status, 0}} ->
        {:ok, output(chunks)}

      {^port, {:exit_status, status}} ->
        {:error, {:exit, status, output(chunks)}}
    after
      remaining_ms ->
        close_port(port)
        {:error, {:timeout, timeout_ms, output(chunks)}}
    end
  end

  defp output(chunks), do: chunks |> Enum.reverse() |> IO.iodata_to_binary()

  defp close_port(port) do
    kill_os_process(port)
    if Port.info(port) != nil, do: Port.close(port)
  rescue
    _ -> :ok
  end

  # Port.close/1 on a :spawn_executable port closes the stdio pipes but
  # leaves the spawned OS process running. For a long-lived child like
  # `ix new` (placement's ix_create_timeout_ms is 120s) the process keeps
  # running well past the timeout and orphans accumulate, so signal the
  # process before closing the port. Mirrors Placement.real_stop_port_forward/1.
  defp kill_os_process(port) do
    case Port.info(port, :os_pid) do
      {:os_pid, os_pid} -> System.cmd("kill", ["-TERM", Integer.to_string(os_pid)], stderr_to_stdout: true)
      nil -> :ok
    end
  end
end
