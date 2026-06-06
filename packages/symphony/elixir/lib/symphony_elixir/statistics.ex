defmodule SymphonyElixir.Statistics do
  @moduledoc "Builds deterministic assignment statistics from GitHub and Linear."

  alias SymphonyElixir.{Config, Linear}

  @github_graphql_endpoint "https://api.github.com/graphql"
  @github_page_size 100

  @github_query """
  query SymphonyStatisticsPullRequests($query: String!, $first: Int!, $after: String) {
    search(type: ISSUE, query: $query, first: $first, after: $after) {
      issueCount
      pageInfo { hasNextPage endCursor }
      nodes {
        ... on PullRequest {
          title
          body
          reviewRequests(first: 20) {
            nodes {
              requestedReviewer {
                ... on User { login avatarUrl }
              }
            }
          }
        }
      }
    }
  }
  """

  @type person_count :: %{
          id: String.t(),
          label: String.t(),
          avatar_url: String.t() | nil,
          count: non_neg_integer()
        }

  @type snapshot :: %{
          github: %{items: [person_count()], total: non_neg_integer(), error: term() | nil},
          linear: %{items: [person_count()], total: non_neg_integer(), error: term() | nil}
        }

  @spec snapshot() :: snapshot()
  def snapshot do
    case github_playbook_prs() do
      {:ok, prs} ->
        identifiers = prs |> Enum.flat_map(&extract_linear_identifiers/1) |> Enum.uniq()

        %{
          github: %{items: count_people(Enum.flat_map(prs, & &1.reviewers)), total: length(prs), error: nil},
          linear: linear_snapshot(identifiers)
        }

      {:error, reason} ->
        %{
          github: %{items: [], total: 0, error: reason},
          linear: %{items: [], total: 0, error: :github_prs_unavailable}
        }
    end
  end

  @spec linear_snapshot([String.t()]) :: %{items: [person_count()], total: non_neg_integer(), error: term() | nil}
  def linear_snapshot(identifiers) when is_list(identifiers) do
    case linear_assignees(identifiers) do
      {:ok, people} -> %{items: count_people(people), total: length(identifiers), error: nil}
      {:error, reason} -> %{items: [], total: length(identifiers), error: reason}
    end
  end

  @spec count_people([map()]) :: [person_count()]
  def count_people(people) when is_list(people) do
    people
    |> Enum.reject(&is_nil/1)
    |> Enum.reduce(%{}, fn person, acc ->
      id = person_id(person)

      Map.update(acc, id, Map.put(person, :count, 1), fn current ->
        %{current | count: current.count + 1}
      end)
    end)
    |> Map.values()
    |> Enum.sort_by(fn %{count: count, label: label} -> {-count, String.downcase(label)} end)
  end

  defp github_playbook_prs, do: github_playbook_prs(nil, [])

  defp github_playbook_prs(after_cursor, acc) do
    config = Config.get()

    with token when is_binary(token) <- config.github_token,
         query when is_binary(query) <- config.github_stats_query,
         {:ok, body} <- github_graphql(token, @github_query, %{query: query, first: @github_page_size, after: after_cursor}) do
      case body do
        %{"data" => %{"search" => %{"nodes" => nodes, "pageInfo" => page_info}}} ->
          prs = Enum.map(nodes, &github_pr/1)
          next_acc = acc ++ prs

          case page_info do
            %{"hasNextPage" => true, "endCursor" => cursor} when is_binary(cursor) ->
              github_playbook_prs(cursor, next_acc)

            _ ->
              {:ok, next_acc}
          end

        %{"errors" => errors} ->
          {:error, {:github_graphql_errors, errors}}

        other ->
          {:error, {:github_unknown_payload, other}}
      end
    else
      nil ->
        cond do
          is_nil(Config.get().github_token) -> {:error, :missing_github_token}
          true -> {:error, :missing_github_stats_query}
        end

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp github_graphql(token, query, variables) do
    Req.post(@github_graphql_endpoint,
      headers: [
        {"authorization", "Bearer " <> token},
        {"accept", "application/vnd.github+json"},
        {"user-agent", "symphony-statistics/0.1.0"}
      ],
      json: %{query: query, variables: variables},
      connect_options: [timeout: 30_000]
    )
    |> case do
      {:ok, %{status: 200, body: body}} -> {:ok, body}
      {:ok, %{status: status, body: body}} -> {:error, {:github_status, status, body}}
      {:error, reason} -> {:error, {:github_request_failed, reason}}
    end
  end

  defp github_pr(%{"title" => title, "body" => body, "reviewRequests" => %{"nodes" => nodes}}) do
    %{title: title || "", body: body || "", reviewers: Enum.map(nodes, &github_review_request/1)}
  end

  defp github_pr(_), do: %{title: "", body: "", reviewers: []}

  defp github_review_request(%{"requestedReviewer" => %{"login" => login} = user}) when is_binary(login) do
    %{id: "github:" <> login, label: login, avatar_url: user["avatarUrl"]}
  end

  defp github_review_request(_), do: nil

  defp extract_linear_identifiers(%{title: title, body: body}) do
    ~r/\bENG-\d+\b/
    |> Regex.scan(title <> "\n" <> body)
    |> List.flatten()
  end

  defp linear_assignees([]), do: {:ok, []}

  defp linear_assignees(identifiers) do
    identifiers
    |> Enum.chunk_every(25)
    |> Enum.reduce_while({:ok, []}, fn chunk, {:ok, acc} ->
      case fetch_linear_issue_chunk(chunk) do
        {:ok, people} -> {:cont, {:ok, acc ++ people}}
        {:error, reason} -> {:halt, {:error, reason}}
      end
    end)
  end

  defp fetch_linear_issue_chunk(identifiers) do
    query = linear_issue_query(identifiers)

    with {:ok, %{"data" => data}} <- Linear.Client.graphql(query, %{}) do
      {:ok, data |> Map.values() |> Enum.map(&linear_person/1) |> Enum.reject(&is_nil/1)}
    else
      {:ok, %{"errors" => errors}} -> {:error, {:linear_graphql_errors, errors}}
      {:ok, other} -> {:error, {:linear_unknown_payload, other}}
      {:error, reason} -> {:error, reason}
    end
  end

  defp linear_issue_query(identifiers) do
    fields =
      identifiers
      |> Enum.with_index()
      |> Enum.map_join("\n", fn {identifier, index} ->
        "i#{index}: issue(id: #{inspect(identifier)}) { assignee { id name displayName avatarUrl } }"
      end)

    "{\n#{fields}\n}"
  end

  defp linear_person(%{"assignee" => %{"id" => id} = assignee}) when is_binary(id) do
    label = assignee["displayName"] || assignee["name"] || id
    %{id: "linear:" <> id, label: label, avatar_url: assignee["avatarUrl"]}
  end

  defp linear_person(_), do: nil

  defp person_id(%{id: id}) when is_binary(id), do: id
end
