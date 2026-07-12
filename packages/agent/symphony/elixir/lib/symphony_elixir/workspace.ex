defmodule SymphonyElixir.Workspace do
  @moduledoc """
  Creates and destroys per-run multi-repo workspaces.

  Layout:

      $SYMPHONY_WORKSPACES_DIR/
        <run_id>/
          <primary>/ primary checkout and Codex cwd
          docs/     sibling checkout
          index/    sibling checkout
          ...

  Repository membership comes from `RepositoryCatalog`. Each checkout has
  local refs and a run-scoped branch, so agents can branch, commit, and open
  PRs in any repository included in the catalog.
  """

  alias SymphonyElixir.Config
  alias SymphonyElixir.PathSafety
  alias SymphonyElixir.Workspace.RepoCloner

  require Logger

  @spec create(String.t()) :: {:ok, Path.t()} | {:error, term()}
  def create(run_id) when is_binary(run_id) do
    config = Config.get()

    if is_binary(config.primary_repo) and not File.dir?(config.primary_repo) do
      {:error, {:primary_repo_not_directory, config.primary_repo}}
    else
      do_create(config, run_id)
    end
  end

  @spec destroy(String.t()) :: :ok
  def destroy(run_id) when is_binary(run_id) do
    config = Config.get()
    path = Path.join(config.workspaces_dir, run_id)

    case canonicalize_under_root(path, config.workspaces_dir) do
      {:ok, canonical} ->
        if File.exists?(canonical), do: File.rm_rf!(canonical)
        :ok

      {:error, reason} ->
        Logger.warning("Refusing to destroy workspace #{path}: #{inspect(reason)}")
        :ok
    end
  end

  defp do_create(config, run_id) do
    workspace_path = Path.join(config.workspaces_dir, run_id)

    with :ok <- ensure_workspace_absent(workspace_path),
         {:ok, canonical} <- canonicalize_under_root(workspace_path, config.workspaces_dir) do
      RepoCloner.clone_all(config, canonical, run_id)
    end
  end

  defp ensure_workspace_absent(path) do
    if File.exists?(path) do
      {:error, {:workspace_already_exists, path}}
    else
      :ok
    end
  end

  defp canonicalize_under_root(path, root) do
    expanded_root = Path.expand(root)
    root_prefix = expanded_root <> "/"

    with {:ok, canonical_path} <- PathSafety.canonicalize(path),
         {:ok, canonical_root} <- PathSafety.canonicalize(expanded_root) do
      canonical_root_prefix = canonical_root <> "/"

      cond do
        String.starts_with?(canonical_path <> "/", canonical_root_prefix) ->
          {:ok, canonical_path}

        String.starts_with?(Path.expand(path) <> "/", root_prefix) ->
          {:error, :symlink_escape}

        true ->
          {:error, {:outside_workspaces_root, canonical_path, canonical_root}}
      end
    end
  end
end
