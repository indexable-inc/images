defmodule SymphonyElixir.Workspace.RepoCloner do
  @moduledoc """
  Clones the repositories that make up one run workspace.

  Repositories are local clones with their own refs and run-scoped branches.
  When a matching checkout exists under the configured local repo root, clone
  with shared objects for speed; otherwise fall back to a shallow GitHub clone.
  """

  alias SymphonyElixir.{Config, RepositoryCatalog}

  @spec clone_all(Config.t(), Path.t(), String.t()) :: {:ok, Path.t()} | {:error, term()}
  def clone_all(%Config{} = config, workspace_path, run_id) when is_binary(workspace_path) do
    primary = RepositoryCatalog.primary(config)

    with :ok <- File.mkdir_p(workspace_path),
         :ok <- clone_repos(config, workspace_path, run_id) do
      {:ok, Path.join(workspace_path, primary.name)}
    end
  end

  defp clone_repos(config, workspace_path, run_id) do
    Enum.reduce_while(RepositoryCatalog.all(config), :ok, fn repo, :ok ->
      case clone_repo(config, workspace_path, repo, run_id) do
        :ok -> {:cont, :ok}
        {:error, reason} -> {:halt, {:error, reason}}
      end
    end)
  end

  defp clone_repo(config, workspace_path, repo, run_id) do
    target = Path.join(workspace_path, repo.name)
    branch = "symphony/#{run_id}"

    with :ok <- ensure_absent(target),
         :ok <- run_git_clone(config, repo, target),
         :ok <- set_origin_url(target, repo),
         :ok <- create_run_branch(target, branch) do
      :ok
    end
  end

  defp ensure_absent(path) do
    case File.exists?(path) do
      false -> :ok
      true -> {:error, {:repo_workspace_already_exists, path}}
    end
  end

  defp run_git_clone(config, repo, target) do
    args =
      case local_checkout(config, repo) do
        {:ok, path} ->
          ["clone", "--local", "--shared", "--branch", repo.default_branch, path, target]

        :error ->
          ["clone", "--depth", "1", "--branch", repo.default_branch, origin_url(repo), target]
      end

    case System.cmd("git", args, stderr_to_stdout: true) do
      {_output, 0} -> :ok
      {output, status} -> {:error, {:git_clone_failed, repo.name, status, String.trim(output)}}
    end
  end

  defp local_checkout(%Config{primary_repo: primary_repo}, %{primary?: true})
       when is_binary(primary_repo) do
    if File.dir?(primary_repo), do: {:ok, primary_repo}, else: :error
  end

  defp local_checkout(%Config{repo_root: root}, repo) when is_binary(root) do
    path = Path.join(root, repo.name)

    if File.dir?(path), do: {:ok, path}, else: :error
  end

  defp local_checkout(_config, _repo), do: :error

  defp set_origin_url(path, repo) do
    case System.cmd("git", ["-C", path, "remote", "set-url", "origin", origin_url(repo)], stderr_to_stdout: true) do
      {_output, 0} -> :ok
      {output, status} -> {:error, {:git_remote_set_url_failed, repo.name, status, String.trim(output)}}
    end
  end

  defp create_run_branch(path, branch) do
    case System.cmd("git", ["-C", path, "checkout", "-b", branch], stderr_to_stdout: true) do
      {_output, 0} -> :ok
      {output, status} -> {:error, {:git_checkout_b_failed, path, status, String.trim(output)}}
    end
  end

  defp origin_url(repo), do: "https://github.com/" <> repo.owner_repo <> ".git"
end
