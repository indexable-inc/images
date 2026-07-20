defmodule IxMcp.NifApp do
  @moduledoc """
  Runtime loader for the unibind-generated NIF OTP apps that ship next to
  the release rather than inside it (packages/tui/ex,
  packages/google/gmail/ex). Such an app is not a mix dep: the release
  bakes the compiled app's store path into an env var (the `IX_MCP_GH`
  pattern) and the facade module loads it onto the code path on first use,
  so the kernel and each binding ship independently.
  """

  @doc """
  Make `module` (the generated namespace, e.g. `TuiEx`) callable: put the
  compiled `app` from `$env_var/ebin` on the code path, load the app, and
  load its modules. Idempotent and race-tolerant: every step tolerates an
  already-loaded app, so concurrent cells can both take this path.
  """
  @spec ensure_loaded(module(), Application.app(), String.t()) :: :ok | {:error, String.t()}
  def ensure_loaded(module, app, env_var) do
    if Code.ensure_loaded?(module) do
      :ok
    else
      load(app, env_var)
    end
  end

  defp load(app, env_var) do
    case System.get_env(env_var) do
      nil ->
        {:error,
         "#{env_var} is not set; it must point at the compiled #{app} OTP app " <>
           "(nix build the binding package, then <out>/lib/#{app})"}

      root ->
        load_from(app, root)
    end
  end

  # The release boots in embedded mode (no code-path autoload), so putting
  # ebin on the path is not enough: load the app and every module of it
  # explicitly. Module load is also what fires the NIF's @on_load, so a
  # broken native library fails here, loudly, not at first call.
  defp load_from(app, root) do
    ebin = Path.join(root, "ebin")

    with true <- Code.append_path(ebin),
         :ok <- load_app(app),
         {:ok, modules} <- :application.get_key(app, :modules),
         :ok <- :code.ensure_modules_loaded(modules) do
      :ok
    else
      failure -> {:error, "loading #{app} from #{root} failed: #{inspect(failure)}"}
    end
  end

  defp load_app(app) do
    case Application.load(app) do
      :ok -> :ok
      {:error, {:already_loaded, ^app}} -> :ok
      {:error, _} = error -> error
    end
  end
end
