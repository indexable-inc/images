defmodule SymphonyElixirWeb.Router do
  @moduledoc "Routes for the runs dashboard and the JSON API."

  use Phoenix.Router
  import Phoenix.LiveView.Router

  pipeline :browser do
    plug(:fetch_session)
    plug(:fetch_live_flash)
    plug(:put_root_layout, html: {SymphonyElixirWeb.Layouts, :root})
    plug(:protect_from_forgery)
    plug(:put_secure_browser_headers)
  end

  pipeline :api do
    plug(:accepts, ["json"])
  end

  scope "/", SymphonyElixirWeb do
    get("/vendor/phoenix/phoenix.js", StaticAssetController, :phoenix)
    get("/vendor/phoenix_html/phoenix_html.js", StaticAssetController, :phoenix_html)
    get("/vendor/phoenix_live_view/phoenix_live_view.js", StaticAssetController, :phoenix_live_view)
  end

  scope "/", SymphonyElixirWeb do
    pipe_through(:browser)

    # The IR runs view is the default dashboard. It carries the
    # schema-driven run control, so there is no separate enqueue form.
    live("/", IRRunsLive, :index)
    live("/ir", IRRunsLive, :index)
    live("/ir/:run_id", IRRunsLive, :show)

    live("/workflows", WorkflowsLive, :index)
    live("/workflows/:name", WorkflowsLive, :show)

    live("/skills", SkillsLive, :index)
    live("/skills/:name", SkillsLive, :show)
    live("/statistics", StatisticsLive, :index)
  end

  scope "/api/v1", SymphonyElixirWeb do
    pipe_through(:api)

    # The manual-trigger producer onto the IR runtime.
    post("/runs", ApiController, :enqueue_run)

    # IR runs (the RunGraph model).
    get("/ir/schema", IRRunController, :schema)
    get("/ir/runs", IRRunController, :index)
    post("/ir/runs", IRRunController, :create)
    get("/ir/runs/:run_id", IRRunController, :show)
    post("/ir/runs/:run_id/cancel", IRRunController, :cancel)
    post("/ir/runs/:run_id/rerun", IRRunController, :rerun)
    post("/ir/runs/:run_id/clear-failed", IRRunController, :clear_failed)
    post("/ir/runs/:run_id/nodes/:node_id/retry", IRRunController, :retry_node)

    post("/triggers/linear", LinearWebhookController, :accept)
    post("/triggers/github", GithubWebhookController, :accept)
    post("/triggers/slack/events", SlackEventsController, :accept)
  end
end
