defmodule SymphonyElixirWeb.StatisticsLive do
  @moduledoc "Statistics dashboard for playbook-created assignments."

  use Phoenix.LiveView

  alias SymphonyElixir.Statistics

  @impl true
  def mount(_params, _session, socket) do
    if connected?(socket) do
      parent = self()

      Task.start(fn ->
        send(parent, {:statistics_snapshot, Statistics.snapshot()})
      end)
    end

    {:ok, assign(socket, loading?: true, snapshot: nil)}
  end

  @impl true
  def handle_info({:statistics_snapshot, snapshot}, socket) do
    {:noreply, assign(socket, loading?: false, snapshot: snapshot)}
  end

  @impl true
  def render(assigns) do
    ~H"""
    {SymphonyElixirWeb.Layouts.app(%{inner_content: render_page(assigns), active_tab: :statistics})}
    """
  end

  defp render_page(%{loading?: true} = assigns) do
    ~H"""
    <div class="empty">loading statistics...</div>
    """
  end

  defp render_page(assigns) do
    ~H"""
    <div class="stats-grid">
      <.chart
        title="GitHub review requests"
        stat={@snapshot.github}
        empty="no requested reviewers found"
      />
      <.chart
        title="Linear assignees from PR tickets"
        stat={@snapshot.linear}
        empty="no ticket assignees found"
      />
    </div>
    """
  end

  defp chart(assigns) do
    assigns =
      assigns
      |> assign(:max_count, max_count(assigns.stat.items))
      |> assign(:error, format_error(assigns.stat.error))

    ~H"""
    <section class="card stats-card">
      <div class="card-header">
        <div>
          <div class="title">{@title}</div>
          <div class="muted mono">{@stat.total} refs scanned</div>
        </div>
      </div>

      <%= cond do %>
        <% @error -> %>
          <div class="empty">{@error}</div>
        <% @stat.items == [] -> %>
          <div class="empty">{@empty}</div>
        <% true -> %>
          <div class="bar-chart">
            <%= for person <- @stat.items do %>
              <div class="bar-row">
                <div class="bar-person">
                  <img src={person.avatar_url || fallback_avatar(person.label)} alt="" loading="lazy" />
                  <span>{person.label}</span>
                </div>
                <div class="bar-track" aria-hidden="true">
                  <div class="bar-fill" style={"width: " <> bar_width(person.count, @max_count)}></div>
                </div>
                <div class="bar-count">{person.count}</div>
              </div>
            <% end %>
          </div>
      <% end %>
    </section>
    """
  end

  defp max_count([]), do: 1
  defp max_count(items), do: items |> Enum.map(& &1.count) |> Enum.max()

  defp bar_width(count, max_count) when max_count > 0 do
    Integer.to_string(round(count / max_count * 100)) <> "%"
  end

  defp fallback_avatar(label) do
    "https://github.com/identicons/" <> URI.encode(label) <> ".png"
  end

  defp format_error(nil), do: nil
  defp format_error(:missing_github_token), do: "GITHUB_TOKEN is not configured."
  defp format_error(:github_prs_unavailable), do: "GitHub PR statistics are not available."
  defp format_error(:missing_linear_api_token), do: "LINEAR_API_KEY is not configured."
  defp format_error(reason), do: "unable to load statistics: " <> inspect(reason)
end
