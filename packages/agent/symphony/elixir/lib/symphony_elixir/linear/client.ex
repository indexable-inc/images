defmodule SymphonyElixir.Linear.Client do
  @moduledoc """
  Thin Linear GraphQL client.

  Two responsibilities:

  - Polling issues with a given label, scoped to a team key.
  - Generic `graphql/2` for the `linear_graphql` tool exposed to skills.

  Auth, endpoint, and team scope come from `Config.get()` at call time.
  No fallback: if `LINEAR_API_KEY` is missing the call returns
  `{:error, :missing_linear_api_token}`.
  """

  alias SymphonyElixir.Config
  alias SymphonyElixir.Linear.Issue

  require Logger

  @issue_page_size 50

  @poll_query """
  query SymphonyPoll($teamKey: String!, $first: Int!, $after: String) {
    issues(filter: {team: {key: {eq: $teamKey}}}, first: $first, after: $after) {
      nodes {
        id
        identifier
        title
        url
        state { name }
        labels { nodes { name } }
      }
      pageInfo { hasNextPage endCursor }
    }
  }
  """

  @spec fetch_issues_with_label(String.t()) :: {:ok, [Issue.t()]} | {:error, term()}
  def fetch_issues_with_label(label) when is_binary(label) do
    config = Config.get()

    cond do
      is_nil(config.linear_api_key) -> {:error, :missing_linear_api_token}
      is_nil(config.linear_team_key) -> {:error, :missing_linear_team_key}
      true -> do_paged_fetch(config, label, nil, [])
    end
  end

  @spec graphql(String.t(), map()) :: {:ok, map()} | {:error, term()}
  def graphql(query, variables \\ %{}) when is_binary(query) and is_map(variables) do
    config = Config.get()

    case config.linear_api_key do
      nil ->
        {:error, :missing_linear_api_token}

      token ->
        payload = %{"query" => query, "variables" => variables}

        case Req.post(config.linear_endpoint,
               headers: [{"Authorization", token}, {"Content-Type", "application/json"}],
               json: payload,
               connect_options: [timeout: 30_000]
             ) do
          {:ok, %{status: 200, body: body}} -> {:ok, body}
          {:ok, %{status: status, body: body}} -> {:error, {:linear_status, status, body}}
          {:error, reason} -> {:error, {:linear_request_failed, reason}}
        end
    end
  end

  defp do_paged_fetch(config, label, after_cursor, acc) do
    variables = %{
      teamKey: config.linear_team_key,
      first: @issue_page_size,
      after: after_cursor
    }

    with {:ok, body} <- graphql(@poll_query, variables) do
      case body do
        %{"data" => %{"issues" => %{"nodes" => nodes, "pageInfo" => page_info}}} ->
          new_issues =
            nodes
            |> Enum.map(&normalize_issue/1)
            |> Enum.filter(fn issue -> not is_nil(issue) and label in issue.labels end)

          next_acc = acc ++ new_issues

          case page_info do
            %{"hasNextPage" => true, "endCursor" => cursor} when is_binary(cursor) ->
              do_paged_fetch(config, label, cursor, next_acc)

            _ ->
              {:ok, next_acc}
          end

        %{"errors" => errors} ->
          {:error, {:linear_graphql_errors, errors}}

        _ ->
          {:error, :linear_unknown_payload}
      end
    end
  end

  defp normalize_issue(%{"id" => id, "identifier" => identifier} = node) do
    %Issue{
      id: id,
      identifier: identifier,
      title: node["title"],
      url: node["url"],
      state: get_in(node, ["state", "name"]),
      labels: extract_labels(node)
    }
  end

  defp normalize_issue(_), do: nil

  defp extract_labels(%{"labels" => %{"nodes" => nodes}}) when is_list(nodes) do
    nodes
    |> Enum.map(& &1["name"])
    |> Enum.reject(&is_nil/1)
    |> Enum.map(&String.downcase/1)
  end

  defp extract_labels(_), do: []
end
