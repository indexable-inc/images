defmodule SymphonyElixirWeb.GithubWebhookController do
  @moduledoc """
  Receives GitHub pull-request label webhooks and starts matching IR runs.

  Only pull_request.labeled events are actionable. Matching is driven by
  `.sym` workflows declaring trigger.kind = github_pr_label with a
  trigger.repo and trigger.label that match the incoming event, resolved
  through the shared `Runtime.Trigger` matcher.
  """

  use Phoenix.Controller, formats: [:json]

  require Logger

  alias SymphonyElixir.Config
  alias SymphonyElixir.Runtime.Ingress

  @spec accept(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def accept(conn, params) do
    with :ok <- verify_signature(conn),
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

  defp verify_signature(conn) do
    cond do
      is_nil(Config.get().github_webhook_secret) ->
        {:error, :unauthorized, "github webhook secret not configured"}

      is_nil(conn.assigns[:raw_body]) ->
        {:error, :bad_request, "missing raw body"}

      true ->
        provided =
          conn
          |> Plug.Conn.get_req_header("x-hub-signature-256")
          |> List.first()

        expected = expected_signature(conn.assigns.raw_body)

        cond do
          is_nil(provided) ->
            {:error, :unauthorized, "missing X-Hub-Signature-256 header"}

          byte_size(provided) != byte_size(expected) ->
            {:error, :unauthorized, "signature mismatch"}

          not Plug.Crypto.secure_compare(provided, expected) ->
            {:error, :unauthorized, "signature mismatch"}

          true ->
            :ok
        end
    end
  end

  defp expected_signature(raw_body) do
    secret = Config.get().github_webhook_secret
    digest = :crypto.mac(:hmac, :sha256, secret, raw_body) |> Base.encode16(case: :lower)
    "sha256=" <> digest
  end

  defp verify_event(conn) do
    case conn |> Plug.Conn.get_req_header("x-github-event") |> List.first() do
      "pull_request" -> :ok
      nil -> {:error, :bad_request, "missing X-GitHub-Event header"}
      other -> {:error, :accepted, "ignored GitHub event #{other}"}
    end
  end

  defp handle_event(%{"action" => "labeled", "pull_request" => pr, "repository" => repo, "label" => label})
       when is_map(pr) and is_map(repo) and is_map(label) do
    repo_name = Map.get(repo, "full_name")
    label_name = label |> Map.get("name", "") |> normalize_label()
    pr_number = Map.get(pr, "number")

    cond do
      Map.get(pr, "state") != "open" ->
        %{ok: true, results: [format_result({:ignored, "PR is not open"})]}

      not is_integer(pr_number) ->
        %{ok: true, results: [format_result({:ignored, "PR number missing"})]}

      active_run_exists?(repo_name, pr_number) ->
        %{ok: true, results: [format_result({:deduped, pr_number})]}

      true ->
        start_label(build_trigger(repo_name, label_name, pr_number, pr), repo_name, pr_number)
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

  defp start_label(trigger, repo_name, pr_number) do
    case Ingress.start_by_trigger(trigger) do
      {:ok, started} ->
        Logger.info("Started runs=#{Enum.map_join(started, ",", & &1.run_id)} for #{repo_name}##{pr_number} via github label")
        %{ok: true, enqueued: length(started), results: Enum.map(started, &format_result({:enqueued, &1.run_id}))}

      {:error, reason} ->
        Logger.warning("Failed to start github label run for #{repo_name}##{pr_number}: #{inspect(reason)}")
        %{ok: true, results: [format_result({:error, inspect(reason)})]}
    end
  end

  defp active_run_exists?(repo, pr_number) do
    Ingress.seen_trigger?(fn
      {status, %{kind: :github_pr_label, repo: r, pr_number: n}} ->
        status in [:pending, :running] and r == repo and n == pr_number

      {_status, _trigger} ->
        false
    end)
  end

  defp normalize_label(name) when is_binary(name), do: name |> String.trim() |> String.downcase()
  defp normalize_label(_), do: ""

  defp format_result({:enqueued, run_id}), do: %{status: "enqueued", run_id: run_id}
  defp format_result({:deduped, pr_number}), do: %{status: "deduped", pr_number: pr_number}
  defp format_result({:ignored, reason}), do: %{status: "ignored", reason: reason}
  defp format_result({:error, reason}), do: %{status: "error", reason: reason}
end
