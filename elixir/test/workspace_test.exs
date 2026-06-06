defmodule SymphonyElixir.WorkspaceTest do
  use ExUnit.Case, async: false

  alias SymphonyElixir.{Config, RepositoryCatalog, Workspace.RepoCloner}

  setup do
    tmp_root = Path.join(System.tmp_dir!(), "symphony_workspace_test_#{System.unique_integer([:positive])}")
    source_root = Path.join(tmp_root, "sources")
    workspaces_dir = Path.join(tmp_root, "workspaces")
    repositories_file = Path.join(tmp_root, "repositories.yaml")

    File.mkdir_p!(source_root)
    File.mkdir_p!(workspaces_dir)

    File.write!(repositories_file, """
    repositories:
      - name: primary-app
        owner_repo: example/primary-app
        default_branch: main
        primary: true
      - name: docs
        owner_repo: example/docs
        default_branch: main
        primary: false
    """)

    local_repos =
      %Config{repositories_file: repositories_file}
      |> RepositoryCatalog.all()
      |> Map.new(fn repo ->
        {repo.name, init_repo!(Path.join(source_root, repo.name), repo.default_branch)}
      end)

    config = %Config{
      primary_repo: Map.fetch!(local_repos, "primary-app"),
      repo_root: source_root,
      repositories_file: repositories_file
    }

    on_exit(fn -> File.rm_rf!(tmp_root) end)

    %{config: config, local_repos: local_repos, workspaces_dir: workspaces_dir}
  end

  test "creates primary workspace with writable sibling repos", %{
    config: config,
    local_repos: local_repos,
    workspaces_dir: workspaces_dir
  } do
    run_root = Path.join(workspaces_dir, "run-1")
    assert {:ok, workspace} = RepoCloner.clone_all(config, run_root, "run-1")

    assert workspace == Path.join([workspaces_dir, "run-1", "primary-app"])
    assert File.exists?(Path.join(workspace, "README.md"))

    docs_repo = Path.join([workspaces_dir, "run-1", "docs"])
    assert File.exists?(Path.join(docs_repo, "README.md"))
    assert {"symphony/run-1\n", 0} = System.cmd("git", ["-C", docs_repo, "branch", "--show-current"])

    assert {alternate, 0} = System.cmd("git", ["-C", docs_repo, "rev-parse", "--git-path", "objects/info/alternates"])
    alternate_path = Path.expand(String.trim(alternate), docs_repo)
    assert File.read!(alternate_path) =~ Path.join(Map.fetch!(local_repos, "docs"), ".git/objects")
  end

  test "primary repo declares main as default", %{config: config} do
    assert %{default_branch: "main", primary?: true} =
             Enum.find(RepositoryCatalog.all(config), & &1.primary?)
  end

  defp init_repo!(path, branch) do
    File.mkdir_p!(path)
    File.write!(Path.join(path, "README.md"), "# #{Path.basename(path)}\n")

    git!(path, ["init", "--initial-branch=#{branch}"])
    git!(path, ["config", "user.name", "Symphony Test"])
    git!(path, ["config", "user.email", "symphony-test@example.com"])
    git!(path, ["add", "README.md"])
    git!(path, ["commit", "-m", "init"])

    path
  end

  defp git!(path, args) do
    case System.cmd("git", ["-C", path] ++ args, stderr_to_stdout: true) do
      {_output, 0} -> :ok
      {output, status} -> flunk("git #{Enum.join(args, " ")} failed with #{status}: #{output}")
    end
  end
end
