defmodule SymphonyElixirWeb.WorkflowsLive do
  @moduledoc """
  Workflow catalog view.

  - `:index` lists every workflow the catalog has loaded with its name and
    trigger label, plus a panel for broken `.sym` files showing their located
    parse diagnostics.
  - `:show` materializes one workflow's AST into a static IR graph and renders
    it with the `IRGraph` component, so an operator can inspect the DAG shape
    without starting a run.

  Reads through `WorkflowCatalog`; hot-reloads on catalog ticks the same way
  `IRRunsLive` re-reads errors on index transitions.
  """

  use Phoenix.LiveView

  alias SymphonyElixir.IR.{Materializer, View}
  alias SymphonyElixir.WorkflowCatalog

  @impl true
  def mount(_params, _session, socket) do
    {:ok,
     socket
     |> assign(workflows: load_workflows())
     |> assign(workflow_errors: load_workflow_errors())}
  end

  @impl true
  def handle_params(%{"name" => name}, _uri, socket) do
    {:noreply, assign(socket, live_action: :show, workflow_name: name)}
  end

  def handle_params(_params, _uri, socket) do
    {:noreply,
     assign(socket,
       live_action: :index,
       workflows: load_workflows(),
       workflow_errors: load_workflow_errors()
     )}
  end

  @impl true
  def render(%{live_action: :show} = assigns) do
    ~H"""
    {SymphonyElixirWeb.Layouts.app(%{inner_content: render_show(assigns), active_tab: :workflows})}
    """
  end

  def render(assigns) do
    ~H"""
    {SymphonyElixirWeb.Layouts.app(%{inner_content: render_index(assigns), active_tab: :workflows})}
    """
  end

  defp render_index(assigns) do
    ~H"""
    <%= if @workflows == [] and @workflow_errors == [] do %>
      <div class="empty">
        no workflows loaded. add a <code class="mono">.sym</code> file under
        <code class="mono">workflows/</code> and the catalog will pick it up within a second.
      </div>
    <% else %>
      <%= if @workflow_errors != [] do %>
        <div class="card">
          <div class="card-header">
            <div class="title">broken workflows</div>
          </div>
          <div class="hint" style="margin-left: 0">
            these <code class="mono">.sym</code> files failed to parse. the last
            working version of each stays loaded; fix the location below and the
            catalog reloads it within a second.
          </div>
          <div class="node-grid">
            <%= for err <- @workflow_errors do %>
              <div class="node-row">
                <div class="mono">{error_location(err)}</div>
                <div><span class="pill failed">parse error</span></div>
                <div class="muted">{err.message}</div>
                <div></div>
              </div>
            <% end %>
          </div>
        </div>
      <% end %>

      <%= if @workflows != [] do %>
        <div class="dag-grid">
          <%= for wf <- @workflows do %>
            <div class="dag-row">
              <div class="name"><a href={"/workflows/" <> wf.name}>{wf.name}</a></div>
              <div class="muted">{trigger_label(wf.trigger)}</div>
              <div></div>
              <div></div>
            </div>
          <% end %>
        </div>
      <% end %>
    <% end %>
    """
  end

  defp render_show(assigns) do
    case Enum.find(assigns.workflows, &(&1.name == assigns.workflow_name)) do
      nil ->
        ~H"""
        <div class="empty">
          no workflow named <span class="mono">{@workflow_name}</span>. <a href="/workflows">back to workflows</a>
        </div>
        """

      entry ->
        assigns =
          assigns
          |> assign(:graph_result, preview_graph(entry))
          |> assign(:workflow_trigger, trigger_label(entry.trigger))

        ~H"""
        <div class="card">
          <div style="display:flex; justify-content:space-between; align-items:baseline">
            <div class="mono">{@workflow_name}</div>
            <div class="muted">{@workflow_trigger}</div>
          </div>
        </div>

        <div class="card">
          <div class="card-header">
            <div class="title">graph</div>
          </div>
          <div class="card-body">
            <%= case @graph_result do %>
              <% {:ok, detail} -> %>
                <SymphonyElixirWeb.Components.IRGraph.graph
                  nodes={detail["nodes"]}
                  trigger={detail["trigger"]}
                  placement={detail["placement"]}
                  base_path="/workflows"
                />
              <% {:error, reason} -> %>
                <div class="empty">cannot preview: {inspect(reason)}</div>
            <% end %>
          </div>
        </div>

        <div><a class="back-link" href="/workflows">&larr; back to workflows</a></div>
        """
    end
  end

  defp preview_graph(entry) do
    case Materializer.materialize("preview-#{entry.name}", entry.hash, entry.ast) do
      {:ok, graph} -> {:ok, View.detail(graph)}
      {:error, reason} -> {:error, reason}
    end
  end

  defp load_workflows do
    WorkflowCatalog.workflows() |> Enum.sort_by(& &1.name)
  end

  defp load_workflow_errors do
    WorkflowCatalog.errors() |> Enum.sort_by(& &1.name)
  end

  # `file:line:column`, the shape an editor jumps to from a build log.
  defp error_location(%{file: file, line: line, column: column}) do
    "#{file}:#{line}:#{column}"
  end

  # Delegate to the shared formatter in View so the form dropdown and the
  # workflows index always show the same label for a given trigger.
  defp trigger_label(trigger), do: View.trigger_label(trigger)
end
