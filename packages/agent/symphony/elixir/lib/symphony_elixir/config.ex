defmodule SymphonyElixir.Config do
  @moduledoc """
  Boot-time snapshot of process-environment configuration.

  Env vars are read once on startup. To pick up changes, restart the BEAM.
  Skill files are hot-reloaded by `SymphonyElixir.Catalog` and workflow
  files by `SymphonyElixir.WorkflowCatalog`; this module owns only the
  values that genuinely require a process restart (network endpoints,
  on-disk paths, polling cadence).

  Required for any work to happen:

      SYMPHONY_ROOT           absolute path of the symphony repo
      SYMPHONY_PRIMARY_REPO   absolute path of the primary repo used as the local source

  Workflow pack selection:

      SYMPHONY_PACK_DIR       absolute path of an external pack directory; takes precedence
                              over SYMPHONY_WORKFLOW_PACK when set
      SYMPHONY_WORKFLOW_PACK  defaults to "example"; selects workflows/<pack> inside the
                              symphony repo when SYMPHONY_PACK_DIR is unset
      SYMPHONY_WORKFLOWS_DIR  defaults to <pack_dir>/workflows
      SYMPHONY_SKILLS_DIR     defaults to <pack_dir>/skills
      SYMPHONY_REPOSITORIES_FILE defaults to <pack_dir>/repositories.yaml

  Runtime paths:

      SYMPHONY_WORKSPACES_DIR defaults to $SYMPHONY_ROOT/workspaces
      SYMPHONY_REPO_ROOT      optional local checkout parent used for fast shared clones.
                              Defaults to the parent directory of SYMPHONY_PRIMARY_REPO.
      SYMPHONY_RUNS_DIR       defaults to $SYMPHONY_ROOT/runs
      SYMPHONY_CODEX_COMMAND  defaults to "codex app-server"
      SYMPHONY_ROOM_SERVER_URL the default room-server base URL for runs whose
                              node placement is `:local` or `:room`. Per-run
                              `:ixvm`/`:host` placements resolve their own URL
                              through `Runtime.Placement`.
      SYMPHONY_ROOM_REGISTRY_URL optional central room-server URL that receives
                              per-VM backend registrations
      SYMPHONY_ROOM_REGISTRY_TOKEN optional bearer token for registry writes
      SYMPHONY_IX_COMMAND     defaults to "ix"
      SYMPHONY_IX_IMAGE       defaults to "ix/symphony-codex:latest"
      SYMPHONY_IX_ROOM_SERVER_COMMAND defaults to "room-server"
      SYMPHONY_IX_REGION      optional; omitted lets ix choose its default
      SYMPHONY_IX_ROOM_PORT   defaults to 8080
      SYMPHONY_IX_ROOM_CONNECT defaults to "direct"; set "port_forward" to
                              tunnel localhost from the Symphony host
      SYMPHONY_IX_LOCAL_PORT_BASE defaults to 18080 for port_forward mode
      SYMPHONY_IX_KEEP_VM     defaults to false; true leaves VMs around after
                              the turn for inspection
      SYMPHONY_IX_CREATE_TIMEOUT_MS defaults to 120000 (2 minutes); the
                              maximum time to wait for `ix new` before the
                              run falls back to the configured placement
                              fallback. Set lower for faster fallback when
                              the ix control plane is degraded.
      SYMPHONY_IX_ENV_PASSTHROUGH comma-separated env names copied into the
                              remote room-server/Codex process (applies to both
                              the ixvm and host runtimes)

  Host placement (a node's `location: host` on the IR engine path):

      SYMPHONY_HOST_USER      OS user the host placement runs codex as.
                              Required for host placement; absent fails setup
                              and retries per SYMPHONY_PLACEMENT_FALLBACK.
      SYMPHONY_HOST_GROUP     optional OS group; omitted uses the user's
                              primary group.
      SYMPHONY_HOST_WORKSPACES_DIR optional parent for run checkouts; defaults
                              to <user home>/symphony-workspaces.
      SYMPHONY_HOST_ROOM_SERVER_COMMAND defaults to "room-server"; the
                              per-run room-server launched as the host user.
      SYMPHONY_HOST_SYSTEMD_RUN_COMMAND defaults to "systemd-run".
      SYMPHONY_HOST_KEEP      defaults to false; true leaves the unit and
                              checkout in place after the turn for inspection.

  Placement fallback (IR engine path):

      SYMPHONY_PLACEMENT_FALLBACK defaults to "host"; the placement a run
                              retries on when its declared `ixvm` placement
                              fails to provision before the first agent turn.
                              "host" reprovisions the per-run room-server as
                              a systemd-run unit on this host; "local" drops
                              to the in-process server (the dev convenience);
                              "none" leaves the run to fail against the
                              missing placement with no fallback.

  Catalog hot-reload:

      SYMPHONY_CATALOG_POLL_MS defaults to 1000

  Integrations:

      LINEAR_API_KEY          enables the Linear graphql tool and webhook enqueue
      LINEAR_TEAM_KEY         optional, used by skills that want to scope queries
      LINEAR_WORKSPACE_SLUG   optional; used to build linear.app issue URLs in
                              dashboards and notifications (e.g. "myorg" yields
                              https://linear.app/myorg/issue/ABC-1)
      LINEAR_WEBHOOK_SECRET   required to accept POST /api/v1/triggers/linear; absent rejects 401
      GITHUB_WEBHOOK_SECRET   required to accept POST /api/v1/triggers/github; absent rejects 401
      GITHUB_TOKEN            enables GitHub-backed dashboard statistics
      SLACK_BOT_OAUTH_TOKEN   enables the Slack huddle trigger; absent disables it
      SLACK_SIGNING_SECRET    required to accept Slack event webhooks
      SYMPHONY_SLACK_POLL_MS  defaults to 60000
      SYMPHONY_SLACK_NOTIFY_CHANNEL optional; set empty to disable post-run notifications
      SYMPHONY_SLACK_NOTIFY_CRON_FAILURES post failed cron runs to Slack; defaults to true
      SYMPHONY_SLACK_NOTIFY_CRON_WORKFLOWS comma-separated workflow names whose cron successes also post, or "*" for every cron success; defaults to none. Notifying runs also post their sink nodes' reserved "slack_summary" output as content (IR.RunNotifier)
      SYMPHONY_ROOM_REGISTRY_URL central room.ix.dev a run's room-server registers with; also the Slack run-detail link base
      SYMPHONY_ROOM_REGISTRY_TOKEN optional bearer token for room backend registration writes
      SYMPHONY_ROOM_ADVERTISE_HOST optional; address a provisioned room-server binds/advertises so room.ix.dev can reach it
      SYMPHONY_ROOM_SERVER_URL optional standing room-server URL for :local / {:room, url} placements
      SYMPHONY_CRON_POLL_MS   defaults to 60000; cadence of the cron trigger tick
      SYMPHONY_CRON_STATE_PATH defaults to runs_dir/cron_state.json
      SYMPHONY_SUBRUN_MAX_DEPTH defaults to 8; the deepest nested-subrun chain a
                              run may spawn before a `subrun` is rejected, the
                              backstop against unbounded recursion that a cycle
                              guard alone cannot catch (mutually recursive but
                              not self-referential workflows)

  GitHub App (optional; when configured, skills push under the App identity):

      SYMPHONY_GITHUB_APP_ID                   numeric GitHub App id. When unset,
                                               skills push under whatever ambient PAT is on PATH.
      SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64   base64 of the App's PEM private key.
      SYMPHONY_GITHUB_APP_OWNER_REPO           repo used for App installation lookup
                                               (e.g. "owner/repo").
      SYMPHONY_BOT_USERNAME                    git user.name to stamp on commits made by
                                               skill nodes (e.g. "my-app[bot]"). Required
                                               when SYMPHONY_GITHUB_APP_ID is set.
      SYMPHONY_BOT_EMAIL                       git user.email to stamp on commits.

  Statistics dashboard (optional):

      SYMPHONY_GITHUB_STATS_QUERY              GitHub search query the statistics dashboard
                                               uses to enumerate bot-authored PRs. Absent
                                               disables the GitHub side of the dashboard.
  """

  use GenServer

  @table :symphony_config

  # Intentional flat config snapshot: each field is one resolved env/opt knob.
  # credo:disable-for-next-line Credo.Check.Warning.StructFieldAmount
  defstruct [
    :root,
    :workflow_pack,
    :pack_dir,
    :primary_repo,
    :workflows_dir,
    :skills_dir,
    :repositories_file,
    :workspaces_dir,
    :repo_root,
    :runs_dir,
    :codex_command,
    # Central room.ix.dev connection settings grouped into one field so the
    # struct stays under the lint's field ceiling: the standing-server URL,
    # the registry URL/token a per-run server registers its backend with, and
    # the host a provisioned server advertises so room.ix.dev can reach it.
    :room,
    :ix_command,
    :ix_image,
    :ix_room_server_command,
    :ix_region,
    :ix_room_port,
    :ix_room_connect,
    :ix_local_port_base,
    :ix_keep_vm?,
    :ix_create_timeout_ms,
    :ix_env_passthrough,
    :host_user,
    :host_group,
    :host_workspaces_dir,
    :host_room_server_command,
    :host_systemd_run_command,
    :host_keep?,
    :placement_fallback,
    # Remote runtime worker connection settings (a worker's identity, the
    # control plane it dials, and the address it binds room-servers on),
    # grouped so the config struct stays under the lint's field ceiling. The
    # `:worker` role is read from the env in Application, not from here.
    :worker,
    :worker_select_label,
    :catalog_poll_ms,
    :linear_api_key,
    :linear_endpoint,
    :linear_team_key,
    :linear_workspace_slug,
    :linear_webhook_secret,
    :github_webhook_secret,
    :github_token,
    :slack_bot_token,
    :slack_signing_secret,
    :slack_endpoint,
    :slack_poll_ms,
    :slack_notify_channel,
    :slack_notify_cron_failures,
    :slack_notify_cron_workflows,
    :cron_state_path,
    :cron_poll_ms,
    :subrun_max_depth,
    :github_app_id,
    :github_app_private_key_pem,
    :github_app_owner_repo,
    :github_app_bot_username,
    :github_app_bot_email,
    :github_stats_query
  ]

  @type t :: %__MODULE__{
          root: Path.t(),
          workflow_pack: String.t(),
          pack_dir: Path.t(),
          primary_repo: Path.t() | nil,
          workflows_dir: Path.t(),
          skills_dir: Path.t(),
          repositories_file: Path.t(),
          workspaces_dir: Path.t(),
          repo_root: Path.t() | nil,
          runs_dir: Path.t(),
          codex_command: String.t(),
          room: %{
            server_url: String.t() | nil,
            registry_url: String.t() | nil,
            registry_token: String.t() | nil,
            advertise_host: String.t() | nil
          },
          ix_command: String.t(),
          ix_image: String.t(),
          ix_room_server_command: String.t(),
          ix_region: String.t() | nil,
          ix_room_port: pos_integer(),
          ix_room_connect: String.t(),
          ix_local_port_base: pos_integer(),
          ix_keep_vm?: boolean(),
          ix_create_timeout_ms: pos_integer(),
          ix_env_passthrough: [String.t()],
          host_user: String.t() | nil,
          host_group: String.t() | nil,
          host_workspaces_dir: Path.t() | nil,
          host_room_server_command: String.t(),
          host_systemd_run_command: String.t(),
          host_keep?: boolean(),
          placement_fallback: :host | :remote | :local | :none,
          worker: %{
            control_plane_url: String.t() | nil,
            worker_id: String.t() | nil,
            worker_labels: [String.t()],
            worker_room_host: String.t() | nil
          },
          worker_select_label: String.t() | nil,
          catalog_poll_ms: pos_integer(),
          linear_api_key: String.t() | nil,
          linear_endpoint: String.t(),
          linear_team_key: String.t() | nil,
          linear_workspace_slug: String.t() | nil,
          linear_webhook_secret: String.t() | nil,
          github_webhook_secret: String.t() | nil,
          github_token: String.t() | nil,
          slack_bot_token: String.t() | nil,
          slack_signing_secret: String.t() | nil,
          slack_endpoint: String.t(),
          slack_poll_ms: pos_integer(),
          slack_notify_channel: String.t() | nil,
          slack_notify_cron_failures: boolean(),
          slack_notify_cron_workflows: [String.t()],
          cron_state_path: Path.t(),
          cron_poll_ms: pos_integer(),
          subrun_max_depth: pos_integer(),
          github_app_id: String.t() | nil,
          github_app_private_key_pem: String.t() | nil,
          github_app_owner_repo: String.t() | nil,
          github_app_bot_username: String.t() | nil,
          github_app_bot_email: String.t() | nil,
          github_stats_query: String.t() | nil
        }

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @spec get() :: t()
  def get do
    case :ets.lookup(@table, :snapshot) do
      [{:snapshot, snapshot}] -> snapshot
      [] -> raise "SymphonyElixir.Config not started"
    end
  end

  @impl true
  def init(opts) do
    :ets.new(@table, [:named_table, :public, read_concurrency: true])

    snapshot = build_snapshot(opts)
    :ok = ensure_dirs!(snapshot)
    :ets.insert(@table, {:snapshot, snapshot})

    {:ok, snapshot}
  end

  # Assembles the ~57-field env snapshot in one pass; each field is one knob.
  # credo:disable-for-next-line Credo.Check.Refactor.CyclomaticComplexity
  defp build_snapshot(opts) do
    root = Keyword.get_lazy(opts, :root, fn -> require_env!("SYMPHONY_ROOT") end)
    root = Path.expand(root)
    workflow_pack = string_env(opts, :workflow_pack, "SYMPHONY_WORKFLOW_PACK", "example")

    primary_repo =
      case Keyword.get(opts, :primary_repo) || System.get_env("SYMPHONY_PRIMARY_REPO") do
        nil -> nil
        value -> Path.expand(value)
      end

    pack_dir =
      path_env(
        opts,
        :pack_dir,
        "SYMPHONY_PACK_DIR",
        Path.join([root, "workflows", workflow_pack])
      )

    workflows_dir = path_env(opts, :workflows_dir, "SYMPHONY_WORKFLOWS_DIR", Path.join(pack_dir, "workflows"))
    skills_dir = path_env(opts, :skills_dir, "SYMPHONY_SKILLS_DIR", Path.join(pack_dir, "skills"))

    repositories_file =
      path_env(opts, :repositories_file, "SYMPHONY_REPOSITORIES_FILE", Path.join(pack_dir, "repositories.yaml"))

    workspaces_dir = path_env(opts, :workspaces_dir, "SYMPHONY_WORKSPACES_DIR", Path.join(root, "workspaces"))
    repo_root = repo_root_env(opts, primary_repo)
    runs_dir = path_env(opts, :runs_dir, "SYMPHONY_RUNS_DIR", Path.join(root, "runs"))

    codex_command = string_env(opts, :codex_command, "SYMPHONY_CODEX_COMMAND", "codex app-server")
    room_server_url = Keyword.get(opts, :room_server_url) || System.get_env("SYMPHONY_ROOM_SERVER_URL")

    room_registry_url =
      empty_to_nil(Keyword.get(opts, :room_registry_url) || System.get_env("SYMPHONY_ROOM_REGISTRY_URL"))

    room_registry_token =
      empty_to_nil(Keyword.get(opts, :room_registry_token) || System.get_env("SYMPHONY_ROOM_REGISTRY_TOKEN"))

    # The address a provisioned per-run room-server binds and advertises so the
    # central room.ix.dev can reach it to proxy reads. Unset keeps the loopback
    # default (only reachable when room.ix.dev shares the host).
    room_advertise_host =
      empty_to_nil(Keyword.get(opts, :room_advertise_host) || System.get_env("SYMPHONY_ROOM_ADVERTISE_HOST"))

    room = %{
      server_url: empty_to_nil(room_server_url),
      registry_url: room_registry_url,
      registry_token: room_registry_token,
      advertise_host: room_advertise_host
    }

    ix_command = string_env(opts, :ix_command, "SYMPHONY_IX_COMMAND", "ix")

    ix_image = string_env(opts, :ix_image, "SYMPHONY_IX_IMAGE", "ix/symphony-codex:latest")

    ix_room_server_command =
      string_env(
        opts,
        :ix_room_server_command,
        "SYMPHONY_IX_ROOM_SERVER_COMMAND",
        "room-server"
      )

    ix_region = empty_to_nil(Keyword.get(opts, :ix_region) || System.get_env("SYMPHONY_IX_REGION"))
    ix_room_port = int_env(opts, :ix_room_port, "SYMPHONY_IX_ROOM_PORT", 8080)
    ix_room_connect = string_env(opts, :ix_room_connect, "SYMPHONY_IX_ROOM_CONNECT", "direct")
    ix_local_port_base = int_env(opts, :ix_local_port_base, "SYMPHONY_IX_LOCAL_PORT_BASE", 18_080)
    ix_keep_vm? = bool_env(opts, :ix_keep_vm?, "SYMPHONY_IX_KEEP_VM", false)
    ix_create_timeout_ms = int_env(opts, :ix_create_timeout_ms, "SYMPHONY_IX_CREATE_TIMEOUT_MS", 120_000)
    ix_env_passthrough = csv_env(opts, :ix_env_passthrough, "SYMPHONY_IX_ENV_PASSTHROUGH", ["OPENAI_API_KEY", "CODEX_API_KEY"])

    host_user = empty_to_nil(Keyword.get(opts, :host_user) || System.get_env("SYMPHONY_HOST_USER"))
    host_group = empty_to_nil(Keyword.get(opts, :host_group) || System.get_env("SYMPHONY_HOST_GROUP"))

    host_workspaces_dir =
      empty_to_nil(Keyword.get(opts, :host_workspaces_dir) || System.get_env("SYMPHONY_HOST_WORKSPACES_DIR"))

    host_room_server_command =
      string_env(opts, :host_room_server_command, "SYMPHONY_HOST_ROOM_SERVER_COMMAND", "room-server")

    host_systemd_run_command =
      string_env(opts, :host_systemd_run_command, "SYMPHONY_HOST_SYSTEMD_RUN_COMMAND", "systemd-run")

    host_keep? = bool_env(opts, :host_keep?, "SYMPHONY_HOST_KEEP", false)
    placement_fallback = placement_fallback_env(opts)

    worker = %{
      control_plane_url: empty_to_nil(Keyword.get(opts, :control_plane_url) || System.get_env("SYMPHONY_CONTROL_PLANE_URL")),
      worker_id: empty_to_nil(Keyword.get(opts, :worker_id) || System.get_env("SYMPHONY_WORKER_ID")),
      worker_labels: csv_env(opts, :worker_labels, "SYMPHONY_WORKER_LABELS", []),
      worker_room_host: empty_to_nil(Keyword.get(opts, :worker_room_host) || System.get_env("SYMPHONY_WORKER_ROOM_HOST"))
    }

    worker_select_label = empty_to_nil(Keyword.get(opts, :worker_select_label) || System.get_env("SYMPHONY_WORKER_SELECT_LABEL"))

    catalog_poll_ms = int_env(opts, :catalog_poll_ms, "SYMPHONY_CATALOG_POLL_MS", 1_000)

    linear_api_key = Keyword.get(opts, :linear_api_key) || System.get_env("LINEAR_API_KEY")
    linear_endpoint = string_env(opts, :linear_endpoint, "LINEAR_API_ENDPOINT", "https://api.linear.app/graphql")
    linear_team_key = Keyword.get(opts, :linear_team_key) || System.get_env("LINEAR_TEAM_KEY")
    linear_workspace_slug = Keyword.get(opts, :linear_workspace_slug) || System.get_env("LINEAR_WORKSPACE_SLUG")

    linear_webhook_secret =
      Keyword.get(opts, :linear_webhook_secret) || System.get_env("LINEAR_WEBHOOK_SECRET")

    github_webhook_secret =
      Keyword.get(opts, :github_webhook_secret) || System.get_env("GITHUB_WEBHOOK_SECRET")

    github_token = Keyword.get(opts, :github_token) || System.get_env("GITHUB_TOKEN") || System.get_env("GH_TOKEN")

    slack_bot_token = Keyword.get(opts, :slack_bot_token) || System.get_env("SLACK_BOT_OAUTH_TOKEN")
    slack_signing_secret = Keyword.get(opts, :slack_signing_secret) || System.get_env("SLACK_SIGNING_SECRET")
    slack_endpoint = string_env(opts, :slack_endpoint, "SLACK_API_ENDPOINT", "https://slack.com/api")
    slack_poll_ms = int_env(opts, :slack_poll_ms, "SYMPHONY_SLACK_POLL_MS", 60_000)

    slack_notify_channel =
      Keyword.get(opts, :slack_notify_channel) ||
        System.get_env("SYMPHONY_SLACK_NOTIFY_CHANNEL")

    slack_notify_cron_failures =
      bool_env(opts, :slack_notify_cron_failures, "SYMPHONY_SLACK_NOTIFY_CRON_FAILURES", true)

    slack_notify_cron_workflows =
      csv_env(opts, :slack_notify_cron_workflows, "SYMPHONY_SLACK_NOTIFY_CRON_WORKFLOWS", [])

    cron_state_path =
      path_env(opts, :cron_state_path, "SYMPHONY_CRON_STATE_PATH", Path.join(runs_dir, "cron_state.json"))

    cron_poll_ms = int_env(opts, :cron_poll_ms, "SYMPHONY_CRON_POLL_MS", 60_000)
    subrun_max_depth = int_env(opts, :subrun_max_depth, "SYMPHONY_SUBRUN_MAX_DEPTH", 8)

    github_app_id =
      empty_to_nil(Keyword.get(opts, :github_app_id) || System.get_env("SYMPHONY_GITHUB_APP_ID"))

    github_app_private_key_pem = load_github_app_private_key(opts)

    github_app_owner_repo =
      empty_to_nil(
        Keyword.get(opts, :github_app_owner_repo) ||
          System.get_env("SYMPHONY_GITHUB_APP_OWNER_REPO")
      )

    github_app_bot_username =
      empty_to_nil(Keyword.get(opts, :github_app_bot_username) || System.get_env("SYMPHONY_BOT_USERNAME"))

    github_app_bot_email =
      empty_to_nil(Keyword.get(opts, :github_app_bot_email) || System.get_env("SYMPHONY_BOT_EMAIL"))

    github_stats_query =
      empty_to_nil(Keyword.get(opts, :github_stats_query) || System.get_env("SYMPHONY_GITHUB_STATS_QUERY"))

    %__MODULE__{
      root: root,
      workflow_pack: workflow_pack,
      pack_dir: pack_dir,
      primary_repo: primary_repo,
      workflows_dir: workflows_dir,
      skills_dir: skills_dir,
      repositories_file: repositories_file,
      workspaces_dir: workspaces_dir,
      repo_root: repo_root,
      runs_dir: runs_dir,
      codex_command: codex_command,
      room: room,
      ix_command: ix_command,
      ix_image: ix_image,
      ix_room_server_command: ix_room_server_command,
      ix_region: ix_region,
      ix_room_port: ix_room_port,
      ix_room_connect: ix_room_connect,
      ix_local_port_base: ix_local_port_base,
      ix_keep_vm?: ix_keep_vm?,
      ix_create_timeout_ms: ix_create_timeout_ms,
      ix_env_passthrough: ix_env_passthrough,
      host_user: host_user,
      host_group: host_group,
      host_workspaces_dir: host_workspaces_dir,
      host_room_server_command: host_room_server_command,
      host_systemd_run_command: host_systemd_run_command,
      host_keep?: host_keep?,
      placement_fallback: placement_fallback,
      worker: worker,
      worker_select_label: worker_select_label,
      catalog_poll_ms: catalog_poll_ms,
      linear_api_key: empty_to_nil(linear_api_key),
      linear_endpoint: linear_endpoint,
      linear_team_key: empty_to_nil(linear_team_key),
      linear_workspace_slug: empty_to_nil(linear_workspace_slug),
      linear_webhook_secret: empty_to_nil(linear_webhook_secret),
      github_webhook_secret: empty_to_nil(github_webhook_secret),
      github_token: empty_to_nil(github_token),
      slack_bot_token: empty_to_nil(slack_bot_token),
      slack_signing_secret: empty_to_nil(slack_signing_secret),
      slack_endpoint: slack_endpoint,
      slack_poll_ms: slack_poll_ms,
      slack_notify_channel: empty_to_nil(slack_notify_channel),
      slack_notify_cron_failures: slack_notify_cron_failures,
      slack_notify_cron_workflows: slack_notify_cron_workflows,
      cron_state_path: cron_state_path,
      cron_poll_ms: cron_poll_ms,
      subrun_max_depth: subrun_max_depth,
      github_app_id: github_app_id,
      github_app_private_key_pem: github_app_private_key_pem,
      github_app_owner_repo: github_app_owner_repo,
      github_app_bot_username: github_app_bot_username,
      github_app_bot_email: github_app_bot_email,
      github_stats_query: github_stats_query
    }
  end

  # Decode SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64 at boot. The base64 form
  # is what sits in secret stores because the secret file is a single-line
  # KEY=VALUE shape that cannot carry the literal PEM newlines. Decode
  # once here and hand the plain PEM string to GithubApp so it never
  # has to re-decode per mint.
  defp load_github_app_private_key(opts) do
    raw =
      Keyword.get(opts, :github_app_private_key_base64) ||
        System.get_env("SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64")

    case empty_to_nil(raw) do
      nil ->
        nil

      base64 ->
        case Base.decode64(base64, ignore: :whitespace) do
          {:ok, pem} ->
            pem

          :error ->
            raise "SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64 is not valid base64"
        end
    end
  end

  defp ensure_dirs!(%__MODULE__{} = snapshot) do
    validate_pack_asset_dir!("SYMPHONY_PACK_DIR", snapshot.pack_dir)
    validate_pack_asset_dir!("SYMPHONY_WORKFLOWS_DIR", snapshot.workflows_dir)
    validate_pack_asset_dir!("SYMPHONY_SKILLS_DIR", snapshot.skills_dir)
    validate_pack_asset_file!("SYMPHONY_REPOSITORIES_FILE", snapshot.repositories_file)

    for dir <- [snapshot.workspaces_dir, snapshot.runs_dir] do
      File.mkdir_p!(dir)
    end

    :ok
  end

  defp validate_pack_asset_dir!(env_name, path) do
    unless File.dir?(path) do
      raise "#{env_name} must point at an existing directory, got #{inspect(path)}"
    end
  end

  defp validate_pack_asset_file!(env_name, path) do
    unless File.regular?(path) do
      raise "#{env_name} must point at an existing file, got #{inspect(path)}"
    end
  end

  defp path_env(opts, key, env_name, default) do
    case Keyword.get(opts, key) || System.get_env(env_name) do
      nil -> default
      "" -> default
      value -> Path.expand(value)
    end
  end

  defp string_env(opts, key, env_name, default) do
    case Keyword.get(opts, key) || System.get_env(env_name) do
      nil -> default
      "" -> default
      value -> value
    end
  end

  defp repo_root_env(opts, primary_repo) do
    case Keyword.get(opts, :repo_root) || System.get_env("SYMPHONY_REPO_ROOT") do
      nil -> if is_binary(primary_repo), do: Path.dirname(primary_repo), else: nil
      "" -> nil
      value -> Path.expand(value)
    end
  end

  defp int_env(opts, key, env_name, default) do
    case Keyword.get(opts, key) || System.get_env(env_name) do
      nil ->
        default

      value when is_binary(value) ->
        case Integer.parse(value) do
          {parsed, ""} when parsed > 0 -> parsed
          _ -> raise "#{env_name} must be a positive integer, got #{inspect(value)}"
        end

      value when is_integer(value) and value > 0 ->
        value
    end
  end

  defp bool_env(opts, key, env_name, default) do
    case Keyword.get(opts, key) do
      value when is_boolean(value) ->
        value

      nil ->
        case System.get_env(env_name) do
          nil -> default
          "" -> default
          value -> parse_bool_env!(env_name, value)
        end
    end
  end

  defp parse_bool_env!(_env_name, value) when value in ["1", "true", "yes", "on"], do: true
  defp parse_bool_env!(_env_name, value) when value in ["0", "false", "no", "off"], do: false

  defp parse_bool_env!(env_name, value) do
    raise "#{env_name} must be boolean-ish, got #{inspect(value)}"
  end

  # The `ixvm -> fallback` target read once at boot. Defaults to :host so a
  # run whose ixvm provisioning fails still completes on a per-run
  # systemd-run room-server rather than aborting; :local is the dev
  # convenience and :none disables the fallback.
  defp placement_fallback_env(opts) do
    case Keyword.get(opts, :placement_fallback) || System.get_env("SYMPHONY_PLACEMENT_FALLBACK") do
      value when value in [:host, :remote, :local, :none] -> value
      value when value in [nil, "", "host"] -> :host
      "remote" -> :remote
      "local" -> :local
      "none" -> :none
      other -> raise "SYMPHONY_PLACEMENT_FALLBACK must be one of host|remote|local|none, got #{inspect(other)}"
    end
  end

  defp csv_env(opts, key, env_name, default) do
    value = Keyword.get(opts, key) || System.get_env(env_name)

    cond do
      is_list(value) ->
        value

      is_binary(value) ->
        value
        |> String.split(",", trim: true)
        |> Enum.map(&String.trim/1)
        |> Enum.reject(&(&1 == ""))

      true ->
        default
    end
  end

  defp require_env!(name) do
    case System.get_env(name) do
      nil -> raise "#{name} must be set"
      "" -> raise "#{name} must not be empty"
      value -> value
    end
  end

  defp empty_to_nil(nil), do: nil
  defp empty_to_nil(""), do: nil
  defp empty_to_nil(value), do: value
end
