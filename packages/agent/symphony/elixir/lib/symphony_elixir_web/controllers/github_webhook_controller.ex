defmodule SymphonyElixirWeb.GithubWebhookController do
  @moduledoc """
  Receives GitHub pull-request label webhooks and starts matching IR runs.

  Only pull_request.labeled events are actionable. Matching is driven by
  `.sym` workflows declaring trigger.kind = github_pr_label with a
  trigger.repo and trigger.label that match the incoming event, resolved
  through the shared `Runtime.Trigger` matcher. Requests are authenticated
  by `SymphonyElixirWeb.WebhookAuth` against `GITHUB_WEBHOOK_SECRET`.
  """

  use Phoenix.Controller, formats: [:json]

  alias SymphonyElixir.Runtime.Ingress
  alias SymphonyElixir.Runtime.Trigger
  alias SymphonyElixirWeb.TriggerResponse
  alias SymphonyElixirWeb.WebhookAuth

  require Logger

  @spec accept(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def accept(conn, params) do
    with :ok <- WebhookAuth.verify(conn, :github),
         :ok <- verify_event(conn) do
      json(conn, handle_event(params))
    else
      {:error, status, reason} ->
        Logger.warning("GitHub webhook rejected: #{reason}")

        conn
        |> put_status(status)
        |> json(%{error: reason})
    end
  end

  defp verify_event(conn) do
    case conn |> Plug.Conn.get_req_header("x-github-event") |> List.first() do
      "pull_request" -> :ok
      nil -> {:error, :bad_request, "missing X-GitHub-Event header"}
      other -> {:error, :accepted, "ignored GitHub event #{other}"}
    end
  end

  defp handle_event(%{"action" => "labeled", "pull_request" => pr, "repository" => repo, "label" => label}) when is_map(pr) and is_map(repo) and is_map(label) do
    repo_name = Map.get(repo, "full_name")
    label_name = label |> Map.get("name", "") |> Trigger.normalize_label()
    pr_number = Map.get(pr, "number")

    cond do
      Map.get(pr, "state") != "open" ->
        %{ok: true, results: [TriggerResponse.format_result({:ignored, "PR is not open"})]}

      not is_integer(pr_number) ->
        %{ok: true, results: [TriggerResponse.format_result({:ignored, "PR number missing"})]}

      active_run_exists?(repo_name, pr_number) ->
        %{ok: true, results: [TriggerResponse.format_result({:deduped, %{pr_number: pr_number}})]}

      true ->
        trigger = build_trigger(repo_name, label_name, pr_number, pr)
        TriggerResponse.start_by_trigger(trigger, "#{repo_name}##{pr_number} via github label")
    end
  end

  defp handle_event(_event), do: %{ok: true, ignored: true}

  defp build_trigger(repo_name, label_name, pr_number, pr) do
    %{
      kind: :github_pr_label,
      repo: repo_name,
      label: label_name,
      pr_number: pr_number,
      pr_url: Map.get(pr, "html_url"),
      title: Map.get(pr, "title"),
      head_ref: get_in(pr, ["head", "ref"]),
      head_repo: get_in(pr, ["head", "repo", "full_name"]),
      base_ref: get_in(pr, ["base", "ref"])
    }
  end

  defp active_run_exists?(repo, pr_number) do
    Ingress.seen_trigger?(fn
      {status, %{kind: :github_pr_label, repo: r, pr_number: n}} ->
        status in [:pending, :running] and r == repo and n == pr_number

      {_status, _trigger} ->
        false
    end)
  end
end
