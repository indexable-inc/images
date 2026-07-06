defmodule SymphonyElixir.Codex.ProvisionTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Codex.Provision
  alias SymphonyElixir.Config
  alias SymphonyElixir.RepositoryCatalog

  defp config_with_repos(extra \\ %{}) do
    dir = Path.join(System.tmp_dir!(), "provision_#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    path = Path.join(dir, "repositories.yaml")

    File.write!(path, """
    repositories:
      - name: app
        owner_repo: acme/app
        default_branch: main
        primary: true
    """)

    on_exit(fn -> File.rm_rf!(dir) end)

    struct(
      Config,
      Map.merge(
        %{
          repositories_file: path,
          github_token: nil,
          ix_env_passthrough: [],
          github_app_bot_username: "acme-bot[bot]",
          github_app_bot_email: "bot@acme.dev"
        },
        extra
      )
    )
  end

  test "sh single-quotes and escapes embedded quotes" do
    assert Provision.sh("plain") == "'plain'"
    assert Provision.sh("a'b") == "'a'\\''b'"
  end

  test "env_export_lines renders a no-op for an empty env" do
    assert Provision.env_export_lines([]) == ":"
  end

  test "env_export_lines quotes values" do
    assert Provision.env_export_lines([{"K", "a b"}]) == "export K='a b'"
  end

  test "runtime_env falls back to the static github_token when no bot token is minted" do
    config = config_with_repos(%{github_token: "ghs_main"})

    env = Provision.runtime_env(config, [])

    assert {"GITHUB_TOKEN", "ghs_main"} in env
    assert {"GH_TOKEN", "ghs_main"} in env
  end

  test "runtime_env: the minted bot token owns both GITHUB_TOKEN and GH_TOKEN over the static token" do
    config = config_with_repos(%{github_token: "ghs_human"})

    env = Provision.runtime_env(config, bot_token: "ghs_app")

    # gh pr create authors as GH_TOKEN, so both vars must carry the App
    # token; neither may fall back to the static host token (ENG-2012).
    assert {"GITHUB_TOKEN", "ghs_app"} in env
    assert {"GH_TOKEN", "ghs_app"} in env
    refute Enum.any?(env, fn {_key, value} -> value == "ghs_human" end)
    keys = Enum.map(env, &elem(&1, 0))
    assert keys == Enum.uniq(keys)
  end

  test "runtime_env: a passthrough of the same name cannot shadow the bot token" do
    var = "GH_TOKEN"
    System.put_env(var, "ghs_inherited")
    on_exit(fn -> System.delete_env(var) end)

    config = config_with_repos(%{github_token: nil, ix_env_passthrough: [var]})

    env = Provision.runtime_env(config, bot_token: "ghs_app")

    assert {"GH_TOKEN", "ghs_app"} in env
    refute {"GH_TOKEN", "ghs_inherited"} in env
  end

  test "repo_blocks stamps the auth header and bot identity when a token is given" do
    config = config_with_repos()
    blocks = Provision.repo_blocks(config, "/home/u/symphony-workspaces/run1", "symphony/run1", "ghs_tok")

    assert blocks =~ "clone --depth 1 --no-checkout --branch 'main' 'https://github.com/acme/app.git'"
    assert blocks =~ "http.https://github.com/.extraheader"
    assert blocks =~ "user.name' 'acme-bot[bot]'"
    assert blocks =~ "user.email' 'bot@acme.dev'"
    assert blocks =~ Base.encode64("x-access-token:ghs_tok")
  end

  test "repo_blocks provisions the workspace as a linked worktree of a hidden base clone" do
    config = config_with_repos()
    blocks = Provision.repo_blocks(config, "/home/u/symphony-workspaces/run1", "symphony/run1", nil)

    # The agent-facing workspace must be a linked worktree, not a
    # standalone clone: repo-side guards treat git-dir == git-common-dir
    # as the human's canonical checkout and deny commits (index#1038).
    assert blocks =~
             "clone --depth 1 --no-checkout --branch 'main' " <>
               "'https://github.com/acme/app.git' '/home/u/symphony-workspaces/run1/.base/app'"

    assert blocks =~
             "git -C '/home/u/symphony-workspaces/run1/.base/app' worktree add " <>
               "'/home/u/symphony-workspaces/run1/app' -b 'symphony/run1'"

    refute blocks =~ "checkout -b"
  end

  test "repo_blocks omits the auth header when no token is available" do
    config = config_with_repos()
    blocks = Provision.repo_blocks(config, "/home/u/symphony-workspaces/run1", "symphony/run1", nil)

    refute blocks =~ "extraheader"
  end

  test "repo_blocks clones an explicit repository list, overriding the config catalog" do
    config = config_with_repos()

    repositories = [
      %RepositoryCatalog{name: "ix", owner_repo: "indexable-inc/ix", default_branch: "main", primary?: true}
    ]

    blocks =
      Provision.repo_blocks(config, "/home/u/symphony-workspaces/run1", "symphony/run1", "ghs_tok", repositories)

    assert blocks =~ "clone --depth 1 --no-checkout --branch 'main' 'https://github.com/indexable-inc/ix.git'"
    refute blocks =~ "acme/app"
  end

  test "host_primary_workspace uses the explicit list's primary, falling back to the config catalog when absent" do
    config = config_with_repos()

    repositories = [
      %RepositoryCatalog{name: "docs", owner_repo: "indexable-inc/docs", default_branch: "main", primary?: false},
      %RepositoryCatalog{name: "ix", owner_repo: "indexable-inc/ix", default_branch: "main", primary?: true}
    ]

    assert Provision.host_primary_workspace(config, "/home/u/symphony-workspaces/run1", "run1", repositories) ==
             "/home/u/symphony-workspaces/run1/ix"

    assert Provision.host_primary_workspace(config, "/home/u/symphony-workspaces/run1", "run1") ==
             "/home/u/symphony-workspaces/run1/app"
  end

  test "backend id and name follow the symphony scheme" do
    assert Provision.backend_id("run1", "impl") == "symphony:run1:impl"
    assert Provision.backend_name(%{identifier: "ENG-1", title: "Do it"}, "run1", "impl") == "ENG-1: Do it / impl"
    assert Provision.backend_name(%{identifier: "ENG-1"}, "run1", "impl") == "ENG-1 / impl"
    assert Provision.backend_name(%{}, "run1", "impl") == "run1 / impl"
  end

  # Redaction and the room-start pkill behavior were asserted through the
  # `Codex.IxVm` / `Codex.Host` delegates before those modules were deleted
  # in the `.sym`/IR cutover. The behavior is owned here, so the coverage
  # moved to the owner.
  test "sanitize_ix_args redacts --env values in ix command args" do
    assert Provision.sanitize_ix_args([
             "new",
             "ix/symphony-codex:2026-05-27",
             "--env",
             "GITHUB_TOKEN=ghs_secret",
             "--env",
             "OPENAI_API_KEY=sk-secret",
             "--name",
             "worker"
           ]) == [
             "new",
             "ix/symphony-codex:2026-05-27",
             "--env",
             "GITHUB_TOKEN=<redacted>",
             "--env",
             "OPENAI_API_KEY=<redacted>",
             "--name",
             "worker"
           ]
  end

  test "sanitize_ix_args redacts sensitive shell exports in ix command args" do
    assert Provision.sanitize_ix_args([
             "shell",
             "worker",
             "--",
             "bash",
             "-lc",
             "export GITHUB_TOKEN='ghs_secret'\nexport OPENAI_API_KEY='sk-secret'\necho ok"
           ]) == [
             "shell",
             "worker",
             "--",
             "bash",
             "-lc",
             "export GITHUB_TOKEN='<redacted>'\nexport OPENAI_API_KEY='<redacted>'\necho ok"
           ]
  end

  test "sanitize_setenv_args redacts --setenv values but keeps other args" do
    args = [
      "--collect",
      "--uid=hari",
      "--setenv=GITHUB_TOKEN=ghs_secret",
      "--setenv=PATH=/usr/bin",
      "--unit=symphony-host-abc.service",
      "--",
      "room-server"
    ]

    assert Provision.sanitize_setenv_args(args) == [
             "--collect",
             "--uid=hari",
             "--setenv=GITHUB_TOKEN=<redacted>",
             "--setenv=PATH=<redacted>",
             "--unit=symphony-host-abc.service",
             "--",
             "room-server"
           ]
  end

  test "ix_room_start_script stops only the room-server process name" do
    script =
      Provision.ix_room_start_script(
        %Config{ix_room_server_command: "room-server", ix_room_port: 8080, github_token: nil, ix_env_passthrough: []},
        "run_test",
        []
      )

    assert script =~ "pkill -x room-server || true"
    refute script =~ "pkill -f room-server"
    # The per-run engine host serves the HTTP /api surface only, so it
    # opts out of the WebTransport listener (room-server #232).
    assert script =~ "--no-wt"
  end

  test "host_room_server_command binds the picked port and disables WebTransport" do
    argv =
      Provision.host_room_server_command(
        %Config{host_room_server_command: "room-server"},
        "127.0.0.1",
        54_321,
        "/home/u/.local/state/room/run1"
      )

    assert argv == [
             System.find_executable("room-server") || "room-server",
             "--host",
             "127.0.0.1",
             "--port",
             "54321",
             "--state-dir",
             "/home/u/.local/state/room/run1",
             "--no-wt"
           ]

    # --no-wt opts out of the WebTransport listener, so per-run host
    # servers do not collide on the fixed UDP port that the standalone
    # server now binds by default (room-server #232).
    assert "--no-wt" in argv
    refute "--wt-port" in argv
  end
end
