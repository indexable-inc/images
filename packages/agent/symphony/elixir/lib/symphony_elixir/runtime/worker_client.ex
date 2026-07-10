defmodule SymphonyElixir.Runtime.WorkerClient do
  @moduledoc """
  Runtime-worker side of the channel: dials the control plane and serves
  provision/teardown by running `Runtime.HostRuntime` on this host.

  Booted only in the `:worker` role. On connect it joins `worker:lobby` with its
  reachable address and labels (carried as URI query params, which the control
  plane's `WorkerSocket.connect/3` reads). The control plane then pushes
  `provision`/`teardown`; each runs in a supervised Task so a minutes-long clone
  or room-server start never blocks the socket's heartbeat, and the result is
  pushed back tagged with the request's `wire_id`.

  The worker holds no secrets: the run's env arrives in the `provision` payload,
  already resolved by the control plane, and is handed straight to HostRuntime.
  """

  use Slipstream

  alias SymphonyElixir.Config
  alias SymphonyElixir.RepositoryCatalog
  alias SymphonyElixir.Runtime.HostRuntime

  require Logger

  @topic "worker:lobby"

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    config = Keyword.get(opts, :config) || Config.get()
    Slipstream.start_link(__MODULE__, config, name: __MODULE__)
  end

  @impl Slipstream
  def init(config) do
    socket =
      new_socket()
      |> assign(:config, config)
      |> assign(:handles, %{})

    {:ok, connect!(socket, connect_opts(config))}
  end

  @impl Slipstream
  def handle_connect(socket) do
    {:ok, join(socket, @topic)}
  end

  @impl Slipstream
  def handle_join(@topic, _response, socket) do
    Logger.info("WorkerClient: joined #{@topic} as #{socket.assigns.config.worker.worker_id}")
    {:ok, socket}
  end

  @impl Slipstream
  def handle_disconnect(_reason, socket) do
    case reconnect(socket) do
      {:ok, socket} -> {:ok, socket}
      {:error, reason} -> {:stop, reason, socket}
    end
  end

  # Control-plane pushes: run the host work in a Task so the socket stays
  # responsive, then a handle_info delivers the result back to push it.
  @impl Slipstream
  def handle_message(@topic, "provision", payload, socket) do
    %{"wire_id" => wire_id, "run_id" => run_id} = payload
    config = with_bot_identity(socket.assigns.config, payload)
    env = wire_env(Map.get(payload, "env", %{}))
    token = Map.get(payload, "bot_token")
    repositories = wire_repositories(Map.get(payload, "repositories", []))
    client = self()

    run_async(fn ->
      result =
        HostRuntime.provision(run_id,
          config: config,
          room_host: config.worker.worker_room_host,
          env: env,
          bot_token: token,
          repositories: repositories
        )

      send(client, {:provision_done, wire_id, run_id, result})
    end)

    {:ok, socket}
  end

  def handle_message(@topic, "teardown", payload, socket) do
    %{"wire_id" => wire_id, "run_id" => run_id} = payload
    config = socket.assigns.config
    {handle, handles} = Map.pop(socket.assigns.handles, run_id)
    client = self()

    run_async(fn ->
      if handle, do: HostRuntime.teardown(handle, config: config)
      send(client, {:teardown_done, wire_id})
    end)

    {:ok, assign(socket, :handles, handles)}
  end

  def handle_message(_topic, _event, _payload, socket), do: {:ok, socket}

  @impl Slipstream
  def handle_info({:provision_done, wire_id, run_id, {:ok, handle}}, socket) do
    push(socket, @topic, "provision_result", %{
      wire_id: wire_id,
      ok: true,
      base_url: handle.base_url,
      primary_workspace: handle.primary_workspace
    })

    {:noreply, assign(socket, :handles, Map.put(socket.assigns.handles, run_id, handle))}
  end

  def handle_info({:provision_done, wire_id, _run_id, {:error, reason}}, socket) do
    push(socket, @topic, "provision_result", %{wire_id: wire_id, ok: false, error: inspect(reason)})
    {:noreply, socket}
  end

  def handle_info({:teardown_done, wire_id}, socket) do
    push(socket, @topic, "teardown_result", %{wire_id: wire_id, ok: true})
    {:noreply, socket}
  end

  defp run_async(fun) do
    Task.Supervisor.start_child(SymphonyElixir.TaskSupervisor, fun)
  end

  # The control plane owns the bot identity (it mints the matching App
  # token), and a worker carries no bot config of its own. Fold the
  # dispatched user.name/user.email onto the run's config so the clone stamps
  # the bot identity; otherwise the checkout keeps the worker host's personal
  # git identity and the babysit skill's identity guard refuses to push. An
  # older control plane that omits the fields leaves the worker config as-is.
  defp with_bot_identity(config, payload) do
    %{
      config
      | github_app_bot_username: present(Map.get(payload, "bot_username")) || config.github_app_bot_username,
        github_app_bot_email: present(Map.get(payload, "bot_email")) || config.github_app_bot_email
    }
  end

  defp present(value) when is_binary(value) and value != "", do: value
  defp present(_), do: nil

  # env crosses the wire as a JSON object; HostRuntime wants a list of
  # {name, value} pairs.
  defp wire_env(env) when is_map(env), do: Enum.map(env, fn {key, value} -> {to_string(key), to_string(value)} end)

  # The control plane sends the run's repository catalog over the channel (the
  # worker holds no pack of its own), so the clone targets the run's real repos
  # rather than the worker's default pack. An empty list (an older control
  # plane that does not send the catalog) falls back to the worker's local
  # config inside `HostRuntime`/`Provision`.
  defp wire_repositories(repositories) when is_list(repositories) do
    case Enum.map(repositories, &wire_repository/1) do
      [] -> nil
      repos -> repos
    end
  end

  defp wire_repository(%{"name" => name, "owner_repo" => owner_repo, "default_branch" => default_branch} = repo) do
    %RepositoryCatalog{
      name: name,
      owner_repo: owner_repo,
      default_branch: default_branch,
      primary?: Map.get(repo, "primary", false) == true
    }
  end

  defp connect_opts(%Config{} = config) do
    [uri: worker_uri(config), reconnect_after_msec: [1_000, 2_000, 5_000, 10_000]]
  end

  # Derive the worker websocket URI from the control-plane base URL, carrying
  # this worker's identity/metadata as query params the socket reads on connect.
  defp worker_uri(%Config{worker: %{control_plane_url: base} = worker}) when is_binary(base) do
    ws_base =
      base
      |> String.replace_prefix("https://", "wss://")
      |> String.replace_prefix("http://", "ws://")
      |> String.trim_trailing("/")

    query =
      URI.encode_query(%{
        "worker_id" => worker.worker_id,
        "address" => worker.worker_room_host,
        "labels" => Enum.join(worker.worker_labels, ","),
        "capacity" => "0"
      })

    "#{ws_base}/worker/websocket?#{query}"
  end
end
