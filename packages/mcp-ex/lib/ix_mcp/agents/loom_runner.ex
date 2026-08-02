defmodule IxMcp.Agents.LoomRunner do
  @moduledoc """
  Runs each ix-mcp Claude subagent inside a snapshot fork of the control VM.

  The first turn snapshots the control VM and restores a dedicated child. A
  later message starts that same stopped VM and resumes its Claude session.
  Every working phase ends by syncing and stopping the child, including error
  returns and runner crashes.
  """

  @behaviour AgentHarness.Runner

  alias AgentHarness.Context
  alias IxMcp.Agents.CliRunner
  alias IxMcp.Agents.Events
  alias IxMcp.Agents.ForkGuard

  @uuid ~r/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/
  @preflight_attempts 30

  @impl true
  def run(instructions, %Context{} = ctx) do
    vm = child_name(ctx.agent_id)

    with :ok <- prepare(vm, Events.session_ref(ctx.agent_id)) do
      try do
        child_ctx = remote_context(ctx, vm)
        CliRunner.run(instructions, child_ctx)
      after
        _ = remote(vm, ["sync"])
        _ = ix(["stop", vm])
      end
    end
  end

  defp prepare(vm, nil) do
    with {:ok, snapshot} <- snapshot(),
         :ok <- ForkGuard.halt_if_fork!(),
         {:ok, _out} <- ix(["new", snapshot, "--name", vm, "--no-shell"] ++ restore_args()),
         :ok <- separate_nodes(vm) do
      case preflight(vm, @preflight_attempts, nil) do
        :ok ->
          :ok

        {:error, _reason} = error ->
          _ = ix(["stop", vm])
          error
      end
    end
  end

  defp prepare(vm, _session), do: ix_ok(["start", vm])

  defp snapshot do
    with {:ok, out} <- ix(["snapshot", parent_vm()]) do
      case Regex.scan(@uuid, out) do
        [] -> {:error, :no_snapshot_id}
        matches -> {:ok, matches |> List.last() |> hd()}
      end
    end
  end

  # The node's public guest endpoint does not hairpin back into a sibling on
  # that same node. Restore may colocate the child with its parent, so move it
  # before the first `ix shell`; migration is an owner-authorized public CLI
  # operation and preserves the restored VM's identity and disk.
  defp separate_nodes(vm) do
    with {:ok, parent_node} <- node_id(parent_vm()),
         {:ok, child_node} <- node_id(vm) do
      maybe_migrate(vm, parent_node, child_node)
    end
  end

  defp maybe_migrate(_vm, parent_node, child_node) when parent_node != child_node, do: :ok

  defp maybe_migrate(vm, parent_node, _child_node) do
    with :ok <- ix_ok(["migrate", vm]),
         {:ok, migrated_node} <- node_id(vm) do
      validate_migration(parent_node, migrated_node)
    end
  end

  defp validate_migration(node, node), do: {:error, {:migration_did_not_separate_nodes, node}}
  defp validate_migration(_parent_node, _migrated_node), do: :ok

  defp node_id(vm) do
    with {:ok, out} <- ix(["vm", "describe", vm, "--output", "json"]),
         {:ok, document} <- json_document(out),
         {:ok, decoded} <- JSON.decode(document) do
      case get_in(decoded, ["placement", "node_id"]) do
        node when is_binary(node) -> {:ok, node}
        _other -> {:error, {:missing_node_id, vm}}
      end
    end
  end

  defp json_document(out) do
    case :binary.match(out, "{") do
      {offset, _length} -> {:ok, binary_part(out, offset, byte_size(out) - offset)}
      :nomatch -> {:error, {:missing_json, out}}
    end
  end

  defp preflight(_vm, 0, last_error), do: {:error, {:preflight, last_error}}

  defp preflight(vm, attempts, _last_error) do
    case remote(vm, ["sh", "-c", preflight_command()]) do
      {:ok, _out} ->
        :ok

      {:error, reason} ->
        Process.sleep(1_000)
        preflight(vm, attempts - 1, reason)
    end
  end

  defp remote_context(ctx, vm) do
    cwd = Keyword.get(ctx.opts, :cwd, IxMcp.Cmd.launch_cwd())

    opts =
      ctx.opts
      |> Keyword.put(:bin, remote_claude())
      |> Keyword.put(:launcher_args, [vm, cwd])

    %{ctx | opts: opts}
  end

  defp remote(vm, command), do: ix(["shell", vm, "--noninteractive", "--" | command])

  defp ix_ok(args) do
    case ix(args) do
      {:ok, _out} -> :ok
      {:error, _reason} = error -> error
    end
  end

  defp ix(args) do
    case System.cmd(ix_bin(), args, stderr_to_stdout: true) do
      {out, 0} -> {:ok, String.trim(out)}
      {out, status} -> {:error, {:exit, status, String.trim(out)}}
    end
  rescue
    ErlangError -> {:error, {:enoent, ix_bin()}}
  end

  defp child_name(id) do
    suffix = id |> String.downcase() |> String.replace(~r/[^a-z0-9-]/, "-")
    "loom-#{suffix}"
  end

  defp restore_args, do: System.get_env("LOOM_RESTORE_ARGS", "") |> String.split()
  defp parent_vm, do: System.fetch_env!("LOOM_PARENT_VM")
  defp ix_bin, do: System.fetch_env!("LOOM_IX_BIN")
  defp remote_claude, do: System.fetch_env!("LOOM_REMOTE_CLAUDE_BIN")
  defp preflight_command, do: System.get_env("LOOM_PREFLIGHT", "true")
end
