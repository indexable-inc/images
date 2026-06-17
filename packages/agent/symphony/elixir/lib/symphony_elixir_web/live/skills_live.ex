defmodule SymphonyElixirWeb.SkillsLive do
  @moduledoc """
  Skill catalog view.

  - `:index` lists every skill the catalog has loaded with its
    description and tools.
  - `:show` renders the full system-prompt body for one skill so the
    operator can read what the agent is being told without leaving the
    dashboard.

  A skill is a model-agnostic prompt body: the execution envelope
  (engine, model, effort, permissions) lives on the workflow `.sym`
  agent node, not the skill, and is shown on the runs view.

  Reads through `Catalog`; hot-reloads when `skills/*.md` changes on
  disk because Catalog re-emits the skill list on its 1s tick.
  """

  use Phoenix.LiveView

  alias SymphonyElixir.Catalog

  @impl true
  def mount(_params, _session, socket) do
    {:ok, assign(socket, skills: Catalog.skills())}
  end

  @impl true
  def handle_params(%{"name" => name}, _uri, socket) do
    skill = Enum.find(socket.assigns.skills, fn s -> s.name == name end)
    {:noreply, assign(socket, live_action: :show, skill: skill, skill_name: name)}
  end

  def handle_params(_params, _uri, socket) do
    {:noreply, assign(socket, live_action: :index)}
  end

  @impl true
  def render(%{live_action: :show} = assigns) do
    ~H"""
    {SymphonyElixirWeb.Layouts.app(%{inner_content: render_show(assigns), active_tab: :skills})}
    """
  end

  def render(assigns) do
    ~H"""
    {SymphonyElixirWeb.Layouts.app(%{inner_content: render_index(assigns), active_tab: :skills})}
    """
  end

  defp render_index(assigns) do
    ~H"""
    <%= if @skills == [] do %>
      <div class="empty">
        no skills loaded. add a file under <code class="mono">skills/</code> and the catalog will pick it up within a second.
      </div>
    <% else %>
      <div class="dag-grid">
        <%= for skill <- @skills do %>
          <div class="dag-row">
            <div class="name"><a href={"/skills/" <> skill.name}>{skill.name}</a></div>
            <div class="muted" title={description_summary(skill.description)}>{description_summary(skill.description)}</div>
            <div class="muted right-align" title={tool_summary(skill.tools)}>{tool_summary(skill.tools)}</div>
          </div>
        <% end %>
      </div>
    <% end %>
    """
  end

  defp render_show(assigns) do
    case assigns.skill do
      nil ->
        ~H"""
        <div class="empty">
          no skill named <span class="mono">{@skill_name}</span>. <a href="/skills">back to skills</a>
        </div>
        """

      _skill ->
        ~H"""
        <div class="card">
          <div style="display:flex; justify-content:space-between; align-items:baseline">
            <div class="mono">{@skill.name}</div>
            <div class="muted mono">{Path.relative_to_cwd(@skill.path)}</div>
          </div>
          <dl class="kv" style="margin-top:12px">
            <dt>description</dt>
            <dd>{description_summary(@skill.description)}</dd>
            <dt>tools</dt>
            <dd class="mono">{tool_summary(@skill.tools)}</dd>
          </dl>
        </div>

        <div class="card">
          <div class="card-header">
            <div class="title">prompt body</div>
          </div>
          <div class="skill-body markdown">{SymphonyElixirWeb.Markdown.to_html(@skill.body)}</div>
        </div>

        <div><a class="back-link" href="/skills">&larr; back to skills</a></div>
        """
    end
  end

  defp tool_summary([]), do: "(no tools)"
  defp tool_summary(tools), do: Enum.join(tools, ", ")

  defp description_summary(nil), do: "(no description)"
  defp description_summary(description) when is_binary(description), do: description
end
