defmodule Loom.Agent do
  @moduledoc """
  One subagent: a GenServer owning one forked VM and one claude child.

  The lifecycle is the state machine the README describes:

      :forking -> :running -> :idle -> :waking -> :running -> ...
                      |                               |
                      +----------- :failed -----------+

  `:forking` covers snapshot + restore (run off-process so the server
  stays responsive), `:running` means a `claude -p` port is streaming,
  `:idle` means the child finished, its final text was delivered, and
  the VM is stopped (disk-only billing). A message to an idle agent
  wakes the VM and resumes the same claude session.

  Every notable transition is sent to the owner as `{:loom, id, event}`:
  `{:spawned, vm}`, `{:final, text}`, `{:failed, reason}`, `:stopped`,
  `:woken`.
  """

  use GenServer, restart: :temporary

  alias Loom.Claude
  alias Loom.Ix

  @enforce_keys [:id, :vm, :brief, :owner, :parent_vm]
  defstruct [
    :id,
    :vm,
    :brief,
    :owner,
    :parent_vm,
    :snapshot_id,
    :session_id,
    :port,
    :result,
    :pending,
    phase: :forking,
    log: []
  ]

  @typedoc "Agent lifecycle phase."
  @type phase :: :forking | :running | :idle | :waking | :failed

  @type t :: %__MODULE__{
          id: String.t(),
          vm: String.t(),
          brief: String.t(),
          owner: pid(),
          parent_vm: String.t(),
          snapshot_id: String.t() | nil,
          session_id: String.t() | nil,
          port: port() | nil,
          result: String.t() | nil,
          pending: String.t() | nil,
          phase: phase(),
          log: [String.t()]
        }

  # Keep only the newest lines of child output for status/debugging; the
  # full stream already went to the owner's dashboard pane.
  @log_keep 200

  @spec start_link({String.t(), String.t(), keyword()}) :: GenServer.on_start()
  def start_link({id, brief, opts}) do
    GenServer.start_link(__MODULE__, {id, brief, opts}, name: via(id))
  end

  @spec via(String.t()) :: {:via, Registry, {Loom.Registry, String.t()}}
  def via(id), do: {:via, Registry, {Loom.Registry, id}}

  @doc "Phase, VM, and result snapshot for `Loom.status/1`."
  @spec status(String.t()) :: {:ok, map()} | {:error, :not_found}
  def status(id) do
    with {:ok, reply} <- call(id, :status), do: {:ok, reply}
  end

  @doc """
  Deliver `text` to the agent.

  Idle agents wake (VM start + session resume). A busy agent refuses
  with `:busy` rather than queueing silently - v1 keeps delivery
  explicit; mailbox semantics belong to the harness layer.
  """
  @spec send_text(String.t(), String.t()) :: :ok | {:error, :busy | :not_found | phase()}
  def send_text(id, text) do
    case call(id, {:send, text}) do
      {:ok, reply} -> reply
      {:error, :not_found} -> {:error, :not_found}
    end
  end

  @doc "Tear down: delete the VM and terminate the agent process."
  @spec delete(String.t()) :: :ok | {:error, :not_found}
  def delete(id) do
    case call(id, :delete, 60_000) do
      {:ok, reply} -> reply
      {:error, :not_found} -> {:error, :not_found}
    end
  end

  # Registry unregistration is asynchronous, so a lookup can return a
  # pid that just exited; treat the resulting `:noproc`/`:normal` exit
  # as the same "no such agent" the empty lookup means.
  @spec call(String.t(), term(), timeout()) :: {:ok, term()} | {:error, :not_found}
  defp call(id, message, timeout \\ 5_000) do
    case Registry.lookup(Loom.Registry, id) do
      [{pid, _value}] ->
        try do
          {:ok, GenServer.call(pid, message, timeout)}
        catch
          :exit, {:noproc, _call} -> {:error, :not_found}
          :exit, {:normal, _call} -> {:error, :not_found}
        end

      [] ->
        {:error, :not_found}
    end
  end

  @impl GenServer
  def init({id, brief, opts}) do
    state = %__MODULE__{
      id: id,
      vm: "loom-#{id}",
      brief: brief,
      owner: Keyword.fetch!(opts, :owner),
      parent_vm: Keyword.fetch!(opts, :parent_vm)
    }

    {:ok, state, {:continue, :provision}}
  end

  @impl GenServer
  def handle_continue(:provision, state) do
    run_async(:provisioned, fn ->
      with {:ok, snapshot_id} <- Ix.snapshot(state.parent_vm),
           # The snapshot may have captured THIS beam; if we are now the
           # resumed clone inside the fork, halt before creating VMs.
           :ok <- Loom.Guard.halt_if_fork!(),
           {:ok, _out} <- Ix.new_from_snapshot(snapshot_id, state.vm),
           :ok <- preflight(state.vm) do
        {:ok, snapshot_id}
      end
    end)

    {:noreply, state}
  end

  # A freshly restored fork needs a moment before its interior is
  # usable (secrets re-materialize, networking settles under the new
  # identity). When `:loom`/`:preflight` is a shell command, retry it
  # in-guest until it exits 0 before launching the child; racing the
  # restore otherwise turns into opaque child failures (measured live:
  # a child at fork+5s spent 48s in auth retries because the secret
  # file was not there yet).
  @preflight_attempts 30
  @preflight_delay_ms 1_000

  @spec preflight(String.t()) :: :ok | {:error, {:preflight, term()}}
  defp preflight(vm) do
    case Application.get_env(:loom, :preflight) do
      nil -> :ok
      command when is_binary(command) -> preflight_loop(vm, command, @preflight_attempts, nil)
    end
  end

  @spec preflight_loop(String.t(), String.t(), non_neg_integer(), term()) ::
          :ok | {:error, {:preflight, term()}}
  defp preflight_loop(_vm, _command, 0, last_error), do: {:error, {:preflight, last_error}}

  defp preflight_loop(vm, command, attempts, _last_error) do
    case Ix.run(["shell", vm, "--noninteractive", "--", "sh", "-c", command]) do
      {:ok, _out} ->
        :ok

      {:error, reason} ->
        Process.sleep(@preflight_delay_ms)
        preflight_loop(vm, command, attempts - 1, reason)
    end
  end

  @impl GenServer
  def handle_call(:status, _from, state) do
    reply = %{
      id: state.id,
      vm: state.vm,
      phase: state.phase,
      session_id: state.session_id,
      result: state.result,
      log_tail: Enum.take(state.log, 10)
    }

    {:reply, reply, state}
  end

  def handle_call({:send, text}, _from, %{phase: :idle, session_id: sid} = state)
      when is_binary(sid) do
    vm = state.vm
    # Same gate as first launch: a cold-started fork needs its interior
    # ready before the resume child runs (measured live: /run/secrets
    # does not exist at all on a restored VM's fresh boot).
    run_async(:started, fn ->
      with {:ok, out} <- Ix.start(vm),
           :ok <- preflight(vm) do
        {:ok, out}
      end
    end)

    {:reply, :ok, %{state | phase: :waking, pending: text}}
  end

  def handle_call({:send, _text}, _from, %{phase: :running} = state) do
    {:reply, {:error, :busy}, state}
  end

  def handle_call({:send, _text}, _from, state) do
    {:reply, {:error, state.phase}, state}
  end

  def handle_call(:delete, _from, state) do
    if is_port(state.port), do: safe_close(state.port)
    _result = Ix.rm(state.vm)
    {:stop, :normal, :ok, %{state | port: nil}}
  end

  @impl GenServer
  def handle_info({:provisioned, {:ok, snapshot_id}}, state) do
    notify(state, {:spawned, state.vm})
    port = Ix.shell_stream(state.vm, Claude.argv(state.brief))
    {:noreply, %{state | phase: :running, snapshot_id: snapshot_id, port: port}}
  end

  def handle_info({:provisioned, {:error, reason}}, state) do
    {:noreply, fail(state, {:provision, reason})}
  end

  def handle_info({:started, {:ok, _out}}, %{phase: :waking, pending: text} = state)
      when is_binary(text) do
    port = Ix.shell_stream(state.vm, Claude.resume_argv(state.session_id, text))
    notify(state, :woken)
    {:noreply, %{state | phase: :running, pending: nil, port: port}}
  end

  def handle_info({:started, {:error, reason}}, %{phase: :waking} = state) do
    {:noreply, fail(state, {:start, reason})}
  end

  def handle_info({port, {:data, {_flag, line}}}, %{port: port} = state) do
    state = %{state | log: Enum.take([line | state.log], @log_keep)}

    case Claude.parse_line(line) do
      {:session, sid} -> {:noreply, %{state | session_id: sid}}
      {:result, text} -> {:noreply, %{state | result: text}}
      _other -> {:noreply, state}
    end
  end

  def handle_info({port, {:exit_status, 0}}, %{port: port} = state) do
    notify(state, {:final, state.result})
    vm = state.vm

    run_async(:vm_stopped, fn ->
      # `ix stop` cuts disk state without draining guest page cache
      # (measured live: the child's session transcript, written ~2s
      # before the stop, came back as a 0-byte file). Force a guest
      # sync first so the session survives for `--resume`.
      _ = Ix.run(["shell", vm, "--noninteractive", "--", "sync"])
      Ix.stop(vm)
    end)

    {:noreply, %{state | phase: :idle, port: nil}}
  end

  def handle_info({port, {:exit_status, status}}, %{port: port} = state) do
    {:noreply, fail(%{state | port: nil}, {:child_exit, status})}
  end

  def handle_info({:vm_stopped, {:ok, _out}}, state) do
    notify(state, :stopped)
    {:noreply, state}
  end

  def handle_info({:vm_stopped, {:error, reason}}, state) do
    # The child already finished and its result was delivered; a failed
    # stop only costs money, so report it without failing the agent.
    notify(state, {:stop_failed, reason})
    {:noreply, state}
  end

  def handle_info({:DOWN, _ref, :process, _pid, :normal}, state), do: {:noreply, state}

  def handle_info({:DOWN, _ref, :process, _pid, reason}, state) do
    {:noreply, fail(state, {:task_crashed, reason})}
  end

  def handle_info(_message, state), do: {:noreply, state}

  # Run `fun` off-process and mail the result back as `{tag, result}`;
  # the monitor turns a crash into `:DOWN` instead of silence.
  @spec run_async(atom(), (-> term())) :: :ok
  defp run_async(tag, fun) do
    server = self()
    {_pid, _ref} = spawn_monitor(fn -> send(server, {tag, fun.()}) end)
    :ok
  end

  # The failed event carries the newest child output: a failed agent's
  # evidence must survive the driver that observed it.
  @spec fail(t(), term()) :: t()
  defp fail(state, reason) do
    notify(state, {:failed, reason, Enum.take(state.log, 25)})
    %{state | phase: :failed}
  end

  @spec notify(t(), term()) :: :ok
  defp notify(state, event) do
    send(state.owner, {:loom, state.id, event})
    :ok
  end

  @spec safe_close(port()) :: :ok
  defp safe_close(port) do
    Port.close(port)
    :ok
  catch
    :error, :badarg -> :ok
  end
end
