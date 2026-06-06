defmodule SymphonyElixir.Runtime.WorkerDispatch do
  @moduledoc """
  Dispatches per-run provision/teardown from the control plane to a runtime
  worker, over the worker's channel process.

  `Runtime.Placement` resolves a worker from `RuntimeRegistry` and asks this
  module to provision or tear down a run on it. The request is delivered to the
  worker's channel process (`worker.pid`) as a message; that process pushes it
  down the worker's WebSocket, awaits the worker's reply, and answers back here.
  This module only talks to a pid, so it carries no dependency on the channel
  implementation and stays unit-testable: `Placement` calls it through its
  driver seam, and tests inject a fake.

  The wire is request/reply correlated by a unique `ref`. A worker that never
  answers within `timeout` yields `{:error, :worker_dispatch_timeout}` rather
  than blocking the run forever.
  """

  @type worker :: SymphonyElixir.Runtime.RuntimeRegistry.worker()

  @typedoc """
  What the worker needs to provision a run's room-server: the runtime env to
  inject (resolved from the control plane's secrets), the bot token for the
  clone, the bot commit identity (`user.name`/`user.email`) that token
  authors as, and the run's repository catalog (resolved from the control
  plane's workflow pack, so the worker clones the run's real repos rather than
  its own default pack). The worker binds the room-server to its own
  configured reachable address, so the bind host is not dictated here.

  `bot_username`/`bot_email` travel here because a worker holds no bot config
  of its own: without them the worker clone keeps its host's personal git
  identity, and the babysit skill's identity guard refuses to push.
  """
  @type spec :: %{
          required(:env) => [{String.t(), String.t()}],
          optional(:bot_token) => String.t() | nil,
          optional(:bot_username) => String.t() | nil,
          optional(:bot_email) => String.t() | nil,
          optional(:repositories) => [SymphonyElixir.RepositoryCatalog.t()]
        }

  @typedoc "The worker's provision result: where to reach the room-server and run."
  @type provisioned :: %{base_url: String.t(), primary_workspace: String.t()}

  @callback provision(worker(), run_id :: String.t(), spec(), timeout()) ::
              {:ok, provisioned()} | {:error, term()}
  @callback teardown(worker(), run_id :: String.t(), timeout()) :: :ok | {:error, term()}

  @doc "Ask `worker` to provision `run_id`'s room-server. See the module doc."
  @spec provision(worker(), String.t(), spec(), timeout()) :: {:ok, provisioned()} | {:error, term()}
  def provision(%{pid: pid}, run_id, spec, timeout) when is_pid(pid) and is_binary(run_id) do
    request(pid, :provision, %{run_id: run_id, spec: spec}, timeout)
  end

  @doc "Ask `worker` to tear down `run_id`'s room-server."
  @spec teardown(worker(), String.t(), timeout()) :: :ok | {:error, term()}
  def teardown(%{pid: pid}, run_id, timeout) when is_pid(pid) and is_binary(run_id) do
    case request(pid, :teardown, %{run_id: run_id}, timeout) do
      {:ok, _} -> :ok
      :ok -> :ok
      {:error, reason} -> {:error, reason}
    end
  end

  defp request(pid, op, payload, timeout) do
    ref = make_ref()
    send(pid, {:runtime_dispatch, op, ref, self(), payload})

    receive do
      {:runtime_dispatch_reply, ^ref, result} -> result
    after
      timeout -> {:error, :worker_dispatch_timeout}
    end
  end
end
