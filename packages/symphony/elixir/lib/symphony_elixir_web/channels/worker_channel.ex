defmodule SymphonyElixirWeb.WorkerChannel do
  @moduledoc """
  Control-plane side of the runtime-worker channel.

  On join, the worker is recorded in `Runtime.RuntimeRegistry` (this channel
  process is the monitored handle, so a disconnect drops the worker). The
  channel then bridges `Runtime.WorkerDispatch`'s in-process request/reply to
  the wire: a `{:runtime_dispatch, op, ref, from, payload}` message is pushed to
  the worker tagged with a `wire_id`, and the worker's `*_result` reply is
  correlated back by that `wire_id` and delivered to the original caller.
  """

  use Phoenix.Channel

  alias SymphonyElixir.Runtime.RuntimeRegistry

  require Logger

  @impl true
  def join("worker:lobby", _payload, socket) do
    assigns = socket.assigns

    if is_binary(assigns.address) and assigns.address != "" do
      :ok =
        RuntimeRegistry.register(%{
          worker_id: assigns.worker_id,
          pid: self(),
          address: assigns.address,
          labels: assigns.labels,
          capacity: assigns.capacity
        })

      Logger.info("WorkerChannel: worker=#{assigns.worker_id} joined address=#{assigns.address}")
      {:ok, assign(socket, :pending, %{})}
    else
      {:error, %{reason: "address required"}}
    end
  end

  # A dispatch from the control plane (WorkerDispatch sent this to our pid).
  # Push it to the worker tagged with a wire id and remember who to answer.
  @impl true
  def handle_info({:runtime_dispatch, op, ref, from, payload}, socket) do
    wire_id = System.unique_integer([:positive])
    push(socket, Atom.to_string(op), wire_payload(op, wire_id, payload))
    {:noreply, assign(socket, :pending, Map.put(socket.assigns.pending, wire_id, {ref, from}))}
  end

  @impl true
  def handle_in("provision_result", payload, socket), do: settle(socket, payload)
  def handle_in("teardown_result", payload, socket), do: settle(socket, payload)

  @impl true
  def terminate(_reason, socket) do
    RuntimeRegistry.unregister(socket.assigns.worker_id)
    :ok
  end

  # env is an in-process keyword-style list of {name, value}; the wire is JSON,
  # so it crosses as a map and the worker rebuilds the list.
  defp wire_payload(:provision, wire_id, %{run_id: run_id, spec: spec}) do
    %{
      wire_id: wire_id,
      run_id: run_id,
      env: Map.new(Map.get(spec, :env, [])),
      bot_token: Map.get(spec, :bot_token),
      bot_username: Map.get(spec, :bot_username),
      bot_email: Map.get(spec, :bot_email),
      repositories: Enum.map(Map.get(spec, :repositories, []), &wire_repository/1)
    }
  end

  defp wire_payload(:teardown, wire_id, %{run_id: run_id}) do
    %{wire_id: wire_id, run_id: run_id}
  end

  # A RepositoryCatalog struct crosses the wire as a plain JSON map; the worker
  # rebuilds the struct from these keys.
  defp wire_repository(%{name: name, owner_repo: owner_repo, default_branch: default_branch, primary?: primary?}) do
    %{name: name, owner_repo: owner_repo, default_branch: default_branch, primary: primary?}
  end

  defp settle(socket, %{"wire_id" => wire_id} = payload) do
    case Map.pop(socket.assigns.pending, wire_id) do
      {nil, _pending} ->
        {:noreply, socket}

      {{ref, from}, pending} ->
        send(from, {:runtime_dispatch_reply, ref, decode_result(payload)})
        {:noreply, assign(socket, :pending, pending)}
    end
  end

  defp decode_result(%{"ok" => true} = payload) do
    {:ok, %{base_url: payload["base_url"], primary_workspace: payload["primary_workspace"]}}
  end

  defp decode_result(%{"ok" => false} = payload), do: {:error, payload["error"] || "worker_error"}
  defp decode_result(_payload), do: {:error, "malformed_worker_reply"}
end
