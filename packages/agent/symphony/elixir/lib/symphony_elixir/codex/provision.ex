defmodule SymphonyElixir.Codex.Provision do
  @moduledoc """
  Shared building blocks for the runtimes that prepare a checkout and a
  room-server outside the BEAM (`Codex.IxVm` and `Codex.Host`).

  Both runtimes clone the same repositories with the same bot-identity and
  GitHub auth stamping, export the same environment into the remote
  room-server/Codex process, and poll the same `/api/health` endpoint. The
  only thing that differs is where the script runs (an iXVM shell versus a
  privilege-dropped local unit). Keeping the clone, env, and health logic
  here means the load-bearing git auth header has a single owner.
  """

  alias SymphonyElixir.Config
  alias SymphonyElixir.RepositoryCatalog

  @ix_workspace_root "/workspace/symphony"
  @ix_room_state_root "/var/lib/symphony-room"

  # The room state and workspace roots the host runtime uses inside the
  # target user's home. Shared so `Codex.Host` and `Runtime.Placement`
  # land the checkout and state in the same place.
  @host_room_state_subdir ".local/state/symphony-room"
  @host_default_workspaces_subdir "symphony-workspaces"

  @doc """
  Shell-quote a value for safe interpolation into a `bash -lc` script.
  """
  @spec sh(String.t()) :: String.t()
  def sh(value) when is_binary(value) do
    "'" <> String.replace(value, "'", "'\\''") <> "'"
  end

  @doc """
  The iXVM-side root that holds every run-scoped checkout, and the
  per-run subdirectory under it. The room-server runs from the primary
  repo's checkout inside this tree.
  """
  @spec ix_run_root(String.t()) :: Path.t()
  def ix_run_root(run_id), do: Path.join(@ix_workspace_root, run_id)

  @doc "The iXVM-side primary-repo checkout for a run, where the engine turn runs."
  @spec ix_primary_workspace(Config.t(), String.t()) :: Path.t()
  def ix_primary_workspace(%Config{} = config, run_id) do
    Path.join(ix_run_root(run_id), RepositoryCatalog.primary(config).name)
  end

  @doc """
  The `ix new` argv that provisions a room-server VM for a run. The
  load-bearing shape (l7-proxy port, region, ipv4, env injection) lives
  here so `Codex.IxVm` and `Runtime.Placement` build it the same way and
  the redaction in `sanitize_ix_args/1` keeps matching it.
  """
  @spec create_vm_args(Config.t(), String.t(), [{String.t(), String.t()}]) :: [String.t()]
  def create_vm_args(%Config{} = config, vm_name, env) when is_binary(vm_name) and is_list(env) do
    ["new", config.ix_image, "--name", vm_name, "--l7-proxy-port", to_string(config.ix_room_port), "--no-shell"]
    |> append_region(config.ix_region)
    |> append_ipv4(config.ix_room_connect)
    |> append_env(env)
  end

  @doc """
  The `bash -lc` script that clones the run's repositories into the VM's
  run root on a run-scoped branch. The caller owns running it through
  `ix shell`.
  """
  @spec ix_workspace_script(Config.t(), String.t(), keyword()) :: String.t()
  def ix_workspace_script(%Config{} = config, run_id, opts) when is_list(opts) do
    token = Keyword.get(opts, :bot_token) || config.github_token
    run_root = ix_run_root(run_id)
    blocks = repo_blocks(config, run_root, "symphony/#{run_id}", token)

    """
    set -euo pipefail
    mkdir -p #{sh(run_root)}
    #{blocks}
    """
  end

  @doc """
  The `bash -lc` script that boots the per-run room-server inside the VM,
  exporting the runtime env first. One owner so `Codex.IxVm` and
  `Runtime.Placement` start the server identically (notably the
  `pkill -x room-server` that stops only the named process).
  """
  @spec ix_room_start_script(Config.t(), String.t(), keyword()) :: String.t()
  def ix_room_start_script(%Config{} = config, run_id, opts) when is_list(opts) do
    room_state_dir = Path.join(@ix_room_state_root, run_id)
    exports = env_export_lines(runtime_env(config, opts))

    """
    set -euo pipefail
    mkdir -p #{sh(room_state_dir)}
    pkill -x room-server || true
    #{exports}
    nohup #{config.ix_room_server_command} --host 0.0.0.0 --port #{config.ix_room_port} --state-dir #{sh(room_state_dir)} --no-wt > /tmp/symphony-room-server.log 2>&1 &
    """
  end

  @doc """
  The `localport:vmport` mapping a port-forward tunnel uses for a VM, and
  the loopback URL that mapping exposes. The local port is derived from
  the VM name so concurrent runs do not collide on the same loopback
  port.
  """
  @spec port_forward_mapping(Config.t(), String.t()) :: {String.t(), String.t()}
  def port_forward_mapping(%Config{} = config, vm_name) when is_binary(vm_name) do
    local_port = config.ix_local_port_base + :erlang.phash2(vm_name, 1000)
    {"#{local_port}:#{config.ix_room_port}", "http://127.0.0.1:#{local_port}"}
  end

  @doc "The `ix port-forward` argv for a VM and `localport:vmport` mapping."
  @spec port_forward_args(String.t(), String.t()) :: [String.t()]
  def port_forward_args(vm_name, mapping) when is_binary(vm_name) and is_binary(mapping) do
    ["port-forward", vm_name, mapping]
  end

  @doc "The `ix rm --force` argv for a VM."
  @spec rm_vm_args(String.t()) :: [String.t()]
  def rm_vm_args(vm_name) when is_binary(vm_name), do: ["rm", "--force", vm_name]

  @doc "The `ix ls --json` argv used to look a VM up by name."
  @spec list_vms_args() :: [String.t()]
  def list_vms_args, do: ["ls", "--json"]

  @doc "The `ix shell <vm> -- bash -lc <script>` argv that runs a setup script in a VM."
  @spec shell_args(String.t(), String.t()) :: [String.t()]
  def shell_args(vm_name, script) when is_binary(vm_name) and is_binary(script) do
    ["shell", vm_name, "--", "bash", "-lc", script]
  end

  @doc """
  A DNS-safe, length-bounded VM name for a run/node, ending in a hash of
  the pair so distinct nodes never collide after slug truncation. Shared
  so the legacy and IR paths name VMs the same way.
  """
  @spec vm_name(String.t(), String.t()) :: String.t()
  def vm_name(run_id, node_id) when is_binary(run_id) and is_binary(node_id) do
    slug =
      "sym-#{run_id}-#{node_id}"
      |> String.downcase()
      |> String.replace(~r/[^a-z0-9-]+/, "-")
      |> String.trim("-")
      |> append_name_hash(run_id, node_id)
      |> String.slice(0, 63)
      |> String.trim("-")

    if slug == "", do: "sym-#{:erlang.unique_integer([:positive])}", else: slug
  end

  @doc """
  Redact secrets from an `ix` argv before it reaches a log or run record.
  Drops the value of any `--env NAME=VALUE` pair and any sensitive
  `export NAME='value'` inside a shell script argument. Shared so both
  the legacy and IR placement paths redact the same way.
  """
  @spec sanitize_ix_args([String.t()]) :: [String.t()]
  def sanitize_ix_args(args) when is_list(args), do: sanitize_ix_args(args, [])

  defp sanitize_ix_args([], acc), do: Enum.reverse(acc)

  defp sanitize_ix_args(["--env", assignment | rest], acc) do
    sanitize_ix_args(rest, [redact_env_assignment(assignment), "--env" | acc])
  end

  defp sanitize_ix_args([arg | rest], acc) do
    sanitize_ix_args(rest, [redact_sensitive_exports(arg) | acc])
  end

  defp redact_env_assignment(assignment) do
    case String.split(assignment, "=", parts: 2) do
      [name, _value] -> name <> "=<redacted>"
      [name] -> name <> "=<redacted>"
    end
  end

  defp redact_sensitive_exports(arg) do
    Regex.replace(~r/export ([A-Z0-9_]*(?:TOKEN|KEY|SECRET)[A-Z0-9_]*)='[^']*'/, arg, "export \\1='<redacted>'")
  end

  defp append_region(args, nil), do: args
  defp append_region(args, ""), do: args
  defp append_region(args, region) when is_binary(region), do: args ++ ["--region", region]

  defp append_ipv4(args, "direct_ipv4"), do: args ++ ["--ipv4"]
  defp append_ipv4(args, _mode), do: args

  defp append_env(args, env) do
    Enum.reduce(env, args, fn {key, value}, acc -> acc ++ ["--env", "#{key}=#{value}"] end)
  end

  defp append_name_hash(slug, run_id, node_id) do
    hash =
      :sha256
      |> :crypto.hash(run_id <> ":" <> node_id)
      |> Base.encode16(case: :lower)
      |> String.slice(0, 10)

    base =
      slug
      |> String.slice(0, 52)
      |> String.trim("-")

    Enum.join([base, hash], "-")
  end

  # --- host (systemd-run) ---------------------------------------------
  #
  # The host runtime drops privileges to `SYMPHONY_HOST_USER` and runs the
  # checkout plus the per-run room-server as transient `systemd-run` units.
  # The argv shape (the `--collect`/`--uid`/`--setenv` base, the named
  # `--unit=`, the sync `--pipe --wait` form) lives here so `Codex.Host`
  # (legacy per-node) and `Runtime.Placement` (IR per-run) build identical
  # commands and the polkit grant keeps matching the unit name.

  @doc """
  The parent of a host run's checkouts: `SYMPHONY_HOST_WORKSPACES_DIR`/run_id
  when set, otherwise `<home>/symphony-workspaces/<run_id>`. The clone lands
  here owned by the target user.
  """
  @spec host_run_root(Config.t(), Path.t(), String.t()) :: Path.t()
  def host_run_root(%Config{host_workspaces_dir: dir}, _home, run_id) when is_binary(dir) and dir != "" do
    Path.join(dir, run_id)
  end

  def host_run_root(%Config{}, home, run_id) when is_binary(home) do
    Path.join([home, @host_default_workspaces_subdir, run_id])
  end

  @doc "The host-side primary-repo checkout for a run, where the engine turn runs."
  @spec host_primary_workspace(Config.t(), Path.t(), String.t(), [RepositoryCatalog.t()] | nil) :: Path.t()
  def host_primary_workspace(%Config{} = config, run_root, _run_id, repositories \\ nil) when is_binary(run_root) do
    primary =
      case repositories do
        nil -> RepositoryCatalog.primary(config)
        repos -> Enum.find(repos, & &1.primary?) || raise "remote provision repositories must define one primary repo"
      end

    Path.join(run_root, primary.name)
  end

  @doc "The per-run room-server state dir under the target user's home."
  @spec host_room_state_dir(Path.t(), String.t()) :: Path.t()
  def host_room_state_dir(home, run_id) when is_binary(home), do: Path.join([home, @host_room_state_subdir, run_id])

  @doc """
  The `systemd-run` unit-name base for a run/node, prefixed with
  `symphony-host-` so the polkit grant (scoped to that prefix in
  `modules/services/symphony`) authorizes the non-root service to manage
  it. The `.service` suffix and any role suffix (`-setup`, `-clean`) are
  the caller's to append.
  """
  @spec host_unit_base(String.t(), String.t()) :: String.t()
  def host_unit_base(run_id, node_id) when is_binary(run_id) and is_binary(node_id) do
    hash =
      :sha256
      |> :crypto.hash(run_id <> ":" <> node_id)
      |> Base.encode16(case: :lower)
      |> String.slice(0, 16)

    "symphony-host-" <> hash
  end

  @doc """
  The `systemd-run` argv that runs `command` to completion as the target
  user via a named transient unit. `--pipe --wait` streams stdio back and
  propagates the exit code; `--collect` reaps the unit even on failure.
  Used for the workspace clone and the cleanup `rm`.
  """
  @spec host_run_sync_args(Config.t(), String.t(), Path.t(), String.t(), [{String.t(), String.t()}], [String.t()]) ::
          [String.t()]
  def host_run_sync_args(%Config{} = config, user, home, unit, env, command) when is_binary(user) and is_binary(home) and is_binary(unit) and is_list(env) and is_list(command) do
    host_base_run_args(config, user, home, env) ++ ["--unit=" <> unit, "--pipe", "--wait", "--"] ++ command
  end

  @doc """
  The `systemd-run` argv that starts a long-lived `command` as the target
  user under a named transient unit and returns once systemd accepts it.
  Used for the per-run room-server; teardown stops the unit by name.
  """
  @spec host_run_unit_args(Config.t(), String.t(), Path.t(), String.t(), [{String.t(), String.t()}], [String.t()]) ::
          [String.t()]
  def host_run_unit_args(%Config{} = config, user, home, unit, env, command) when is_binary(user) and is_binary(home) and is_binary(unit) and is_list(env) and is_list(command) do
    host_base_run_args(config, user, home, env) ++ ["--unit=" <> unit, "--"] ++ command
  end

  defp host_base_run_args(%Config{host_group: group}, user, home, env) do
    setenv = Enum.map(env, fn {key, value} -> "--setenv=#{key}=#{value}" end)

    ["--collect", "--uid=" <> user, "--working-directory=" <> home]
    |> host_append_group(group)
    |> Kernel.++(setenv)
  end

  defp host_append_group(args, group) when is_binary(group) and group != "", do: args ++ ["--gid=" <> group]
  defp host_append_group(args, _group), do: args

  @doc """
  The `bash -lc` script that prepares a host run: makes the room state and
  run-root dirs, then clones the run's repositories on a run-scoped branch.
  The caller owns running it through a `systemd-run --pipe --wait` unit.
  """
  @spec host_workspace_script(
          Config.t(),
          Path.t(),
          Path.t(),
          String.t(),
          String.t() | nil,
          [RepositoryCatalog.t()] | nil
        ) :: String.t()
  def host_workspace_script(%Config{} = config, run_root, state_dir, run_id, token, repositories \\ nil) when is_binary(run_root) and is_binary(state_dir) and is_binary(run_id) do
    blocks = repo_blocks(config, run_root, "symphony/#{run_id}", token, repositories)

    """
    set -euo pipefail
    mkdir -p #{sh(state_dir)} #{sh(run_root)}
    #{blocks}
    """
  end

  @doc "The `rm -rf <run_root>` script for the host cleanup unit."
  @spec host_cleanup_script(Path.t()) :: String.t()
  def host_cleanup_script(run_root) when is_binary(run_root), do: "rm -rf #{sh(run_root)}"

  @doc """
  The room-server argv for the host runtime: the configured command split
  on whitespace (its head resolved to an absolute path) plus the bind
  host/port and state dir. The room-server runs on loopback only; the
  caller picks the port. `--no-wt` opts out of the WebTransport listener:
  a host-placed engine host only serves the HTTP `/api` surface, and the
  fixed WT port would collide across the many per-run servers that share
  one host.
  """
  @spec host_room_server_command(Config.t(), String.t(), pos_integer(), Path.t()) :: [String.t()]
  def host_room_server_command(%Config{host_room_server_command: command}, host, port, state_dir) when is_binary(host) and is_integer(port) and is_binary(state_dir) do
    [exe | rest] =
      case String.split(command, ~r/\s+/, trim: true) do
        [head | rest] -> [System.find_executable(head) || head | rest]
        [] -> ["room-server"]
      end

    [exe | rest] ++
      ["--host", host, "--port", Integer.to_string(port), "--state-dir", state_dir, "--no-wt"]
  end

  @doc """
  Parse the target user's home directory out of a `getent passwd` line.
  Shared so both host paths resolve the same `$HOME` the checkout and room
  state live under.
  """
  @spec parse_passwd_home(String.t(), String.t()) :: {:ok, Path.t()} | {:error, term()}
  def parse_passwd_home(output, user) when is_binary(output) and is_binary(user) do
    output
    |> String.split("\n", trim: true)
    |> List.first()
    |> case do
      nil ->
        {:error, {:host_user_unknown, user}}

      line ->
        case String.split(line, ":") do
          fields when length(fields) >= 6 ->
            home = Enum.at(fields, 5)
            if is_binary(home) and home != "", do: {:ok, home}, else: {:error, {:host_user_no_home, user}}

          _ ->
            {:error, {:host_user_unknown, user}}
        end
    end
  end

  @doc """
  Redact secrets from a `systemd-run` argv before it reaches a log or run
  record: drop the value of any `--setenv=NAME=value` pair. Shared so the
  legacy and IR host paths redact the same way.
  """
  @spec sanitize_setenv_args([String.t()]) :: [String.t()]
  def sanitize_setenv_args(args) when is_list(args) do
    Enum.map(args, fn arg ->
      case String.split(arg, "=", parts: 3) do
        ["--setenv", name, _value] -> "--setenv=" <> name <> "=<redacted>"
        _ -> arg
      end
    end)
  end

  @doc """
  Provision every repository in the active catalog into `run_root`, on a
  run-scoped `branch`, stamping the bot identity and (when `token` is
  present) a GitHub Basic auth header so plain `git push` authors as the
  App. Each workspace is a linked worktree of a hidden base clone under
  `run_root/.base`, not a standalone clone: repo-side guards distinguish
  a human's canonical checkout from an agent worktree by comparing
  `git-dir` to `git-common-dir`, and a standalone clone is misclassified
  as the canonical checkout, denying the run's own commits on its
  sanctioned branch (index#1038). The base clone is `--no-checkout` so a
  run carries one working tree per repo, not two. Returns the
  concatenated `bash` blocks; the caller owns the surrounding
  `set -euo pipefail` and the `mkdir -p` of `run_root`.
  """
  @spec repo_blocks(Config.t(), Path.t(), String.t(), String.t() | nil, [RepositoryCatalog.t()] | nil) :: String.t()
  def repo_blocks(%Config{} = config, run_root, branch, token, repositories \\ nil) do
    basic = if is_binary(token), do: Base.encode64("x-access-token:" <> token)

    Enum.map_join(repositories || RepositoryCatalog.all(config), "\n", &clone_repo_block(&1, run_root, branch, basic, config))
  end

  defp clone_repo_block(repo, run_root, branch, basic, config) do
    target = Path.join(run_root, repo.name)
    base = Path.join([run_root, ".base", repo.name])
    remote = "https://github.com/#{repo.owner_repo}.git"
    clone_auth = if is_binary(basic), do: "-c http.https://github.com/.extraheader=#{sh("Authorization: Basic " <> basic)}", else: ""

    # `git config --local` from a linked worktree writes to the base
    # clone's shared config, so the header and identity stamped on the
    # worktree below cover every push and commit made from it.
    extraheader =
      if is_binary(basic),
        do: "git -C #{sh(target)} config --local http.https://github.com/.extraheader #{sh("Authorization: Basic " <> basic)}",
        else: ":"

    """
    rm -rf #{sh(target)} #{sh(base)}
    git #{clone_auth} clone --depth 1 --no-checkout --branch #{sh(repo.default_branch)} #{sh(remote)} #{sh(base)}
    git -C #{sh(base)} worktree add #{sh(target)} -b #{sh(branch)}
    #{git_identity_lines(target, config)}
    #{extraheader}
    """
  end

  defp git_identity_lines(target, %Config{} = config) do
    [
      git_config_line(target, "user.name", config.github_app_bot_username),
      git_config_line(target, "user.email", config.github_app_bot_email)
    ]
    |> Enum.reject(&(&1 == nil))
    |> case do
      [] -> ":"
      lines -> Enum.join(lines, "\n")
    end
  end

  defp git_config_line(_target, _key, nil), do: nil
  defp git_config_line(_target, _key, ""), do: nil

  defp git_config_line(target, key, value) do
    "git -C #{sh(target)} config --local #{sh(key)} #{sh(value)}"
  end

  @doc """
  The environment the remote room-server (and the Codex process it spawns)
  needs: the bot `GITHUB_TOKEN`/`GH_TOKEN`, then any names listed in
  `SYMPHONY_IX_ENV_PASSTHROUGH` copied from the Symphony host.

  The GitHub token is `opts[:bot_token]` when the runtime minted a GitHub
  App installation token for the run, falling back to the static
  `config.github_token` only when no App token is available. `gh pr create`
  authors as whoever owns `GH_TOKEN` regardless of the workspace
  `user.email`, so the same `:bot_token` that stamps the clone auth header
  in `repo_blocks/4` must own the room-server's `GITHUB_TOKEN`/`GH_TOKEN`;
  otherwise an agent PR is authored by the static host token. The GitHub
  vars are placed before the passthrough so a `SYMPHONY_IX_ENV_PASSTHROUGH`
  entry of the same name cannot shadow the bot identity.
  """
  @spec runtime_env(Config.t(), keyword()) :: [{String.t(), String.t()}]
  def runtime_env(%Config{} = config, opts) when is_list(opts) do
    token = Keyword.get(opts, :bot_token) || config.github_token

    github_env =
      case token do
        t when is_binary(t) and t != "" -> [{"GITHUB_TOKEN", t}, {"GH_TOKEN", t}]
        _ -> []
      end

    passthrough =
      Enum.flat_map(config.ix_env_passthrough, fn name ->
        case System.get_env(name) do
          nil -> []
          "" -> []
          value -> [{name, value}]
        end
      end)

    Enum.uniq_by(github_env ++ passthrough, fn {key, _value} -> key end)
  end

  @doc """
  Render `export KEY='value'` lines for a `bash -lc` script, or `:` (a
  no-op) when the environment is empty so the surrounding script stays
  valid.
  """
  @spec env_export_lines([{String.t(), String.t()}]) :: String.t()
  def env_export_lines([]), do: ":"

  def env_export_lines(env) do
    Enum.map_join(env, "\n", fn {key, value} -> "export #{key}=#{sh(value)}" end)
  end

  @type context :: %{optional(:identifier) => String.t(), optional(:title) => String.t()}

  @doc "Stable registry id for a run's node-scoped room backend."
  @spec backend_id(String.t(), String.t()) :: String.t()
  def backend_id(run_id, node_id), do: "symphony:#{run_id}:#{node_id}"

  @doc "Human-facing backend name shown in the room backend picker."
  @spec backend_name(context(), String.t(), String.t()) :: String.t()
  def backend_name(%{identifier: id, title: title}, _run_id, node_id) when is_binary(id) and is_binary(title) do
    "#{id}: #{title} / #{node_id}"
  end

  def backend_name(%{identifier: id}, _run_id, node_id) when is_binary(id), do: "#{id} / #{node_id}"
  def backend_name(_context, run_id, node_id), do: "#{run_id} / #{node_id}"

  @doc """
  Poll `<url>/api/health` until it answers 2xx or `timeout_ms` elapses.
  """
  @spec wait_for_room(String.t(), pos_integer()) :: :ok | {:error, term()}
  def wait_for_room(url, timeout_ms) do
    deadline = System.monotonic_time(:millisecond) + timeout_ms
    do_wait_for_room(url, deadline, nil)
  end

  defp do_wait_for_room(url, deadline, last_error) do
    if System.monotonic_time(:millisecond) >= deadline do
      {:error, {:room_health_timeout, url, last_error}}
    else
      case Req.get(url <> "/api/health", receive_timeout: 2_000, connect_options: [timeout: 2_000]) do
        {:ok, %{status: status}} when status in 200..299 ->
          :ok

        {:ok, %{status: status, body: body}} ->
          Process.sleep(1_000)
          do_wait_for_room(url, deadline, {:status, status, body})

        {:error, reason} ->
          Process.sleep(1_000)
          do_wait_for_room(url, deadline, reason)
      end
    end
  end
end
