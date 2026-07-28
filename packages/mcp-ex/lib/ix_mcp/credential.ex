defmodule IxMcp.Credential do
  @moduledoc """
  Run a command with a borrowed GitHub credential, when one is on loan.

  ## Why this exists

  A fleet host has no git credential of its own. GitHub refuses public-key
  authentication from the fleet's addresses while accepting the same key
  from a workstation, so a forwarded ssh agent does not help: it only
  signs. The supported workarounds all leave the token behind, in
  `.git/config`, in a `nix.conf`, or in the process table of a box several
  people share.

  `ix-credential` carries the token the way `ssh -A` carries a key: the
  workstation holds it and answers questions about it over a unix socket,
  the socket is forwarded into the ssh session, and the loan ends when the
  session does. Nothing is copied, so nothing has to be cleaned up.

  ## Why the kernel needs its own seam

  A shell inherits the forwarded socket's path from its ssh session. This
  kernel does not: it is started once and outlives every session, so there
  is no session to inherit from. The path is therefore **derived** rather
  than inherited (see `socket_path/0`), which lets three parties that never
  talk to each other agree on it: the workstation binding the socket, git's
  credential helper on the borrowing host, and this process.

  ## What this exposes, and what it does not

  `run/3` runs a command with git wired to the loan. It deliberately does
  **not** hand back a credential, and there is no `token/0` to ask for one.
  The question a caller actually has is "run my build, which needs GitHub",
  not "give me a secret", and answering the narrower question means the
  token never enters this process's memory, never reaches a log, and cannot
  be captured by a cell that only meant to check something.

  `status/0` is the companion, and is diagnostic only: it answers whether a
  loan is live without running anything, so "why did my build fail" has an
  answer that is not another failed build.

      Credential.status()
      #=> {:error, "no credential loan is running: /run/ix-credential/0.sock does not exist"}

      Credential.run("nix", ["build", ".#thing"], cd: repo)
      #=> {"", 0}
  """

  alias IxMcp.Cmd

  @socket_env "IX_CREDENTIAL_SOCK"
  @helper_binary "ix-credential"

  # Kept in step with `packages/ix-credential/src/socket.rs` by
  # `test/ix_mcp/credential_test.exs`, which asks the binary itself. Two
  # copies of a three-line rule beats a subprocess on every call, but only
  # if something fails when they drift.
  @runtime_dir "/run/ix-credential"

  @doc """
  The socket path this host derives, unless `IX_CREDENTIAL_SOCK` overrides
  it.

  Linux puts it in a shared, sticky `#{@runtime_dir}` so each uid binds its
  own; darwin, which is the lending side and single-user, uses the per-user
  temp directory.
  """
  @spec socket_path() :: binary()
  def socket_path do
    case System.get_env(@socket_env) do
      path when is_binary(path) and path != "" -> path
      _ -> derived_socket_path()
    end
  end

  @doc """
  Whether a credential loan is live, and where.

  The three ways this can be "no" are three different situations for the
  operator, so they are three different messages rather than one.
  """
  @spec status() :: {:ok, binary()} | {:error, binary()}
  def status do
    path = socket_path()

    cond do
      not File.exists?(path) ->
        {:error,
         "no credential loan is running: #{path} does not exist. " <>
           "Lend one from your workstation with `ix-credential lend <this-host>`."}

      not socket?(path) ->
        {:error, "#{path} exists but is not a socket, so nothing can be lending on it."}

      is_nil(System.find_executable(@helper_binary)) ->
        {:error,
         "a loan is bound at #{path} but #{@helper_binary} is not on PATH, " <>
           "so git has no helper to ask. Install it on this host."}

      true ->
        {:ok, path}
    end
  end

  @doc """
  Run `cmd` with git wired to the credential loan.

  Options and return match `IxMcp.Cmd.run/3`. A caller's own `env:` is
  merged rather than replaced, and an existing `GIT_CONFIG_COUNT` is
  appended to rather than clobbered, so this composes with a caller that
  already sets git config through the environment.

  Raises when no loan is live: a command that needs a credential and
  silently runs without one fails later, further from the cause, with
  github's message rather than ours.
  """
  @spec run(binary(), [binary()], keyword()) :: {Collectable.t(), non_neg_integer()}
  def run(cmd, args \\ [], opts \\ []) do
    case status() do
      {:error, reason} -> raise reason
      {:ok, path} -> Cmd.run(cmd, args, Keyword.put(opts, :env, credential_env(path, opts)))
    end
  end

  @doc """
  The environment `run/3` adds, for a caller that must spawn its own way.

  Exposed for the rare spawn this module cannot make (a port, a `Fleet`
  call), not as the normal path: everything routed through `run/3` gets the
  liveness check for free, and this does not.
  """
  @spec credential_env(binary(), keyword()) :: [{binary(), binary()}]
  def credential_env(path, opts \\ []) do
    inherited = Keyword.get(opts, :env, [])
    helper = System.find_executable(@helper_binary)
    index = git_config_count(inherited)

    # `put_env` and not `++`: a second entry for a key already in the
    # caller's list leaves the port to pick between them, and which one it
    # picks is not something this should depend on. `GIT_CONFIG_COUNT` is
    # the case that matters, because the caller almost always already has
    # one when there is anything to append to.
    Enum.reduce(
      [
        {@socket_env, path},
        # git treats a helper value containing a space as a command line, so
        # the absolute path and the subcommand ride together.
        {"GIT_CONFIG_KEY_#{index}", "credential.helper"},
        {"GIT_CONFIG_VALUE_#{index}", "#{helper} helper"},
        {"GIT_CONFIG_COUNT", to_string(index + 1)},
        # Without this a helper that declines leaves git waiting on a
        # terminal that a kernel spawn does not have.
        {"GIT_TERMINAL_PROMPT", "0"}
      ],
      inherited,
      fn {key, value}, env -> List.keystore(env, key, 0, {key, value}) end
    )
  end

  # Append rather than overwrite: a caller already passing git config
  # through the environment keeps theirs and gets ours.
  @spec git_config_count([{binary(), binary()}]) :: non_neg_integer()
  defp git_config_count(inherited) do
    existing =
      case List.keyfind(inherited, "GIT_CONFIG_COUNT", 0) do
        {_key, value} -> value
        nil -> System.get_env("GIT_CONFIG_COUNT")
      end

    case existing && Integer.parse(existing) do
      {count, ""} when count >= 0 -> count
      _ -> 0
    end
  end

  @spec derived_socket_path() :: binary()
  defp derived_socket_path do
    uid = System.get_env("UID") || os_uid()

    case :os.type() do
      {:unix, :darwin} -> Path.join(System.tmp_dir!(), "ix-credential-#{uid}.sock")
      _ -> Path.join(@runtime_dir, "#{uid}.sock")
    end
  end

  @spec os_uid() :: binary()
  defp os_uid do
    {out, 0} = Cmd.run("id", ["-u"])
    String.trim(out)
  end

  @spec socket?(binary()) :: boolean()
  defp socket?(path) do
    match?({:ok, %File.Stat{type: :other}}, File.stat(path))
  end
end
