defmodule SymphonyElixirWeb.IRRunsLive do
  @moduledoc """
  Dashboard LiveView over IR runs (the `RunGraph` model), the live-page
  counterpart to the read-only `IRRunController` JSON API.

  Two actions:

  - `:index` - every IR run as a table, plus a "run" control that starts a
    workflow from the `WorkflowCatalog` by name.
  - `:show` - one run in detail with per-node state pills, mirroring the
    `node-grid` layout the legacy `RunsLive` uses.

  Live updates ride `Runtime.Events`: `Runtime` broadcasts an
  `{:ir_run_event, run_id, summary}` on every persisted transition. This
  LiveView subscribes to the index topic on mount and to the open run's
  topic when navigating to `:show`, so pills move from running to succeeded
  with no polling. The data shapes come straight from `IR.View` so the page
  renders the same facts the JSON API serves.
  """

  use Phoenix.LiveView

  alias SymphonyElixir.IR.{Store, View}
  alias SymphonyElixir.Runtime.Events
  alias SymphonyElixir.{Runtime, WorkflowCatalog}

  # The runs table paginates at this many rows per page, navigated via the
  # `?page=N` query param. The full sorted list still loads on every render
  # (the store scan is cheap) so the total count and "latest first" order
  # stay exact; only the rendered slice is bounded.
  @per_page 50

  @impl true
  def mount(_params, _session, socket) do
    # The index topic carries every run's transitions, so a subscriber on
    # the connected mount can refresh the table from the event payload. The
    # first (static) render runs disconnected; skip the subscribe there.
    if connected?(socket), do: Events.subscribe_index()

    {:ok,
     socket
     |> assign(runs: load_runs())
     |> assign(workflows: load_workflows())
     |> assign(workflow_errors: load_workflow_errors())
     |> assign(subscribed_run: nil)
     |> assign(page: 1)
     |> assign(path: "/")
     |> assign(form_error: nil)}
  end

  @impl true
  def handle_params(%{"run_id" => run_id}, _uri, socket) do
    socket = resubscribe_run(socket, run_id)

    detail =
      case Store.load(run_id) do
        {:ok, graph} -> View.detail(graph)
        {:error, _} -> nil
      end

    {:noreply, assign(socket, live_action: :show, run_id: run_id, detail: detail)}
  end

  def handle_params(params, uri, socket) do
    socket = resubscribe_run(socket, nil)

    {:noreply,
     assign(socket,
       live_action: :index,
       page: parse_page(params["page"]),
       path: URI.parse(uri).path,
       runs: load_runs(),
       workflows: load_workflows(),
       workflow_errors: load_workflow_errors()
     )}
  end

  @impl true
  def handle_info({:ir_run_event, run_id, _summary}, %{assigns: %{live_action: :show, run_id: run_id}} = socket) do
    # A transition on the open run: re-read the store for the full detail
    # view (the event payload is only the summary, and the node grid needs
    # per-node state). A read miss leaves the last-good detail in place.
    detail =
      case Store.load(run_id) do
        {:ok, graph} -> View.detail(graph)
        {:error, _} -> socket.assigns[:detail]
      end

    {:noreply, assign(socket, detail: detail)}
  end

  def handle_info({:ir_run_event, _run_id, _summary}, %{assigns: %{live_action: :index}} = socket) do
    # Any run transitioned: refresh the index table. Re-reading the store
    # rather than splicing the one summary keeps sort order and the
    # appearance of a brand-new run consistent without per-row bookkeeping.
    # Re-read the catalog's parse errors on the same beat so the broken-
    # workflow panel reflects a hot-reload that landed between navigations.
    {:noreply, assign(socket, runs: load_runs(), workflow_errors: load_workflow_errors())}
  end

  def handle_info({:ir_run_event, _run_id, _summary}, socket), do: {:noreply, socket}

  @impl true
  def handle_event("run", %{"workflow" => name}, socket) when is_binary(name) and name != "" do
    case Runtime.Ingress.start_by_name(name, %{kind: :manual, input: %{}}, []) do
      {:ok, %{run_id: run_id}} ->
        {:noreply, push_navigate(socket, to: "/ir/" <> run_id)}

      {:error, reason} ->
        {:noreply, assign(socket, form_error: "could not start #{name}: #{inspect(reason)}")}
    end
  end

  def handle_event("run", _params, socket) do
    {:noreply, assign(socket, form_error: "pick a workflow to run")}
  end

  def handle_event("cancel", _params, %{assigns: %{run_id: id}} = socket) do
    try do
      _ = Runtime.cancel(id, "dashboard")
    catch
      :exit, _ -> :ok
    end

    {:noreply, assign(socket, detail: reload_detail(id))}
  end

  def handle_event("retry_failed", _params, %{assigns: %{run_id: id}} = socket) do
    try do
      _ = Runtime.clear_failed(id, "dashboard")
    catch
      :exit, _ -> :ok
    end

    {:noreply, assign(socket, detail: reload_detail(id))}
  end

  def handle_event("rerun", _params, %{assigns: %{run_id: id}} = socket) do
    try do
      _ = Runtime.rerun(id, "dashboard")
    catch
      :exit, _ -> :ok
    end

    {:noreply, assign(socket, detail: reload_detail(id))}
  end

  @impl true
  def render(%{live_action: :show} = assigns) do
    ~H"""
    {SymphonyElixirWeb.Layouts.app(%{inner_content: render_show(assigns), active_tab: :ir})}
    """
  end

  def render(assigns) do
    ~H"""
    {SymphonyElixirWeb.Layouts.app(%{inner_content: render_index(assigns), active_tab: :ir})}
    """
  end

  defp render_index(assigns) do
    # Bound the rendered slice to one page. `page` is clamped against the
    # live total so a stale `?page=N` (or a run count that shrank under the
    # current page) falls back to the last real page instead of an empty
    # table.
    total = length(assigns.runs)
    total_pages = max(1, div(total + @per_page - 1, @per_page))
    page = assigns.page |> max(1) |> min(total_pages)
    page_runs = assigns.runs |> Enum.drop((page - 1) * @per_page) |> Enum.take(@per_page)

    assigns =
      assigns
      |> assign(:page, page)
      |> assign(:total_pages, total_pages)
      |> assign(:total_runs, total)
      |> assign(:page_runs, page_runs)
      |> assign(:per_page, @per_page)

    ~H"""
    <div class="toolbar">
      <button class="btn btn-primary" popovertarget="run-launcher">start a run</button>
    </div>

    <div popover id="run-launcher" class="launcher-popover">
      <div class="launcher-title">start a run</div>
      <%= if @workflows == [] do %>
        <div class="muted">
          no <code class="mono">.sym</code> workflows loaded. drop a file under
          <code class="mono">workflows/</code> and the catalog will pick it up within a second.
        </div>
      <% else %>
        <form class="enqueue" phx-submit="run">
          <div class="row">
            <label for="ir-workflow">workflow</label>
            <select id="ir-workflow" name="workflow" class="field-input">
              <%= for wf <- @workflows do %>
                <option value={wf.name}>{wf.name} - {trigger_label(wf.trigger)}</option>
              <% end %>
            </select>
          </div>
          <%= if @form_error do %>
            <div class="hint" style="color: var(--bad); margin-left: 0">{@form_error}</div>
          <% end %>
          <div class="submit-row">
            <button class="btn btn-primary" type="submit">run</button>
          </div>
        </form>
      <% end %>
    </div>

    <%= if @runs == [] do %>
      <div class="empty">no IR runs yet. start one with the button above.</div>
    <% else %>
      <div class="card">
        <div class="card-header">
          <div class="title">{run_count_label(@runs)}</div>
        </div>
        <table class="runs">
          <thead>
            <tr>
              <th>run</th>
              <th>status</th>
              <th>nodes</th>
              <th>cost</th>
              <th>updated</th>
            </tr>
          </thead>
          <tbody>
            <%= for run <- @page_runs do %>
              <tr>
                <td class="mono"><a href={"/ir/" <> run["run_id"]}>{run["run_id"]}</a></td>
                <td><span class={"pill " <> run["status"]}>{run["status"]}</span></td>
                <td>{node_counts(run)}</td>
                <td class="muted">{cost_label(run["cost_usd"])}</td>
                <td class="muted">{relative_time(run["updated_at"])}</td>
              </tr>
            <% end %>
          </tbody>
        </table>
        <%= if @total_pages > 1 do %>
          <div class="pager">
            <div>
              showing {(@page - 1) * @per_page + 1}-{min(@page * @per_page, @total_runs)} of {@total_runs}
            </div>
            <div class="pages">
              <.link
                class={if @page <= 1, do: "disabled", else: ""}
                patch={page_path(@path, @page - 1)}
              >prev</.link>
              <span class="page-num current">{@page} / {@total_pages}</span>
              <.link
                class={if @page >= @total_pages, do: "disabled", else: ""}
                patch={page_path(@path, @page + 1)}
              >next</.link>
            </div>
          </div>
        <% end %>
      </div>
    <% end %>

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
    """
  end

  defp render_show(assigns) do
    case assigns.detail do
      nil ->
        ~H"""
        <div class="empty">run not found. <a href="/ir">back to IR runs</a></div>
        """

      _detail ->
        ~H"""
        <div class="card">
          <div class="card-header">
            <div class="title">run</div>
            <span class={"pill " <> @detail["status"]}>{@detail["status"]}</span>
          </div>
          <div class="card-body">
            <dl class="kv">
              <dt>run id</dt><dd class="mono">{@detail["run_id"]}</dd>
              <dt>trigger</dt><dd>{@detail["trigger"]}</dd>
              <dt>placement</dt><dd>{placement_label(@detail["placement"])}</dd>
              <dt>nodes</dt><dd>{detail_node_counts(@detail)}</dd>
              <dt>cost</dt><dd class="mono">{cost_label(@detail["cost_usd"])}</dd>
              <dt>started</dt><dd class="muted">{@detail["created_at"] || "-"}</dd>
            </dl>
          </div>
        </div>

        <div class="card">
          <div class="card-header">
            <div class="title">graph</div>
          </div>
          <div class="card-body">
            <SymphonyElixirWeb.Components.IRGraph.graph
              nodes={@detail["nodes"]}
              trigger={@detail["trigger"]}
              placement={@detail["placement"]}
            />
          </div>
        </div>

        <div class="card">
          <div class="card-header">
            <div class="title">nodes</div>
            <div class="actions">
              <%= if @detail["status"] in ["pending", "running"] do %>
                <button class="btn" phx-click="cancel">cancel run</button>
              <% end %>
              <%= if @detail["status"] in ["failed"] do %>
                <button class="btn" phx-click="retry_failed">retry failed</button>
                <button class="btn" phx-click="rerun">rerun</button>
              <% end %>
            </div>
          </div>
          <div class="node-grid">
            <%= for node <- @detail["nodes"] do %>
              <div class="node-row">
                <div class="mono">{node["id"]}</div>
                <div>
                  <span class={"pill " <> node["state"]}>{node["state"]}</span>
                </div>
                <div class="muted mono">{node["kind"]}{engine_label(node["envelope"])}</div>
                <div class="muted">{node_cost(node)}</div>
              </div>
            <% end %>
          </div>
        </div>

        <div><a class="back-link" href="/ir">&larr; back to IR runs</a></div>
        """
    end
  end

  # Keep at most one per-run subscription alive as the operator navigates
  # between detail pages. Switching from one run to another drops the old
  # topic so the LiveView is not woken by transitions on a run it no longer
  # shows; the index topic (subscribed once at mount) is left untouched.
  defp resubscribe_run(socket, run_id) do
    if connected?(socket) do
      current = socket.assigns[:subscribed_run]

      if current != run_id do
        if is_binary(current), do: Phoenix.PubSub.unsubscribe(SymphonyElixir.PubSub, Events.run_topic(current))
        if is_binary(run_id), do: Events.subscribe_run(run_id)
        assign(socket, subscribed_run: run_id)
      else
        socket
      end
    else
      socket
    end
  end

  defp reload_detail(run_id) do
    case Store.load(run_id) do
      {:ok, graph} -> View.detail(graph)
      {:error, _} -> nil
    end
  end

  defp load_runs do
    # Latest first: the most recently updated run leads the table, matching
    # the "updated" column. `sort_by/3` with `:desc` puts newest at the top;
    # the run_id is a stable tiebreaker for runs that share a timestamp.
    Store.load_all()
    |> Enum.map(&View.summary/1)
    |> Enum.sort_by(&{&1["updated_at"], &1["run_id"]}, :desc)
  end

  # `?page=N` is operator-supplied, so anything that is not a positive
  # integer (absent, empty, negative, garbage) falls back to the first page.
  # The upper bound is clamped against the live total in render_index.
  defp parse_page(raw) when is_binary(raw) do
    case Integer.parse(raw) do
      {n, _} when n > 0 -> n
      _ -> 1
    end
  end

  defp parse_page(_), do: 1

  # Keep page 1 on the bare path so the canonical first-page URL has no query
  # string; later pages carry `?page=N` on whichever index path is active
  # (`/` or `/ir`).
  defp page_path(path, page) when page <= 1, do: path
  defp page_path(path, page), do: path <> "?page=" <> Integer.to_string(page)

  defp load_workflows do
    WorkflowCatalog.workflows() |> Enum.sort_by(& &1.name)
  end

  defp load_workflow_errors do
    WorkflowCatalog.errors() |> Enum.sort_by(& &1.name)
  end

  # `file:line:column`, the shape an editor jumps to from a build log. The
  # diagnostic always carries a file basename, so the location is enough to
  # find the offending token without a byte offset.
  defp error_location(%{file: file, line: line, column: column}) do
    "#{file}:#{line}:#{column}"
  end

  defp run_count_label(runs) do
    count = length(runs)
    if count == 1, do: "1 run", else: "#{count} runs"
  end

  defp node_counts(%{"states" => states}) when is_map(states) do
    total = states |> Map.values() |> Enum.sum()
    done = Map.get(states, "succeeded", 0)
    "#{done}/#{total}"
  end

  defp node_counts(_), do: "0/0"

  # Richer node-count summary for the run detail header: each non-zero state
  # is shown so the operator can see "1 succeeded - 2 running - 1 pending"
  # at a glance without scrolling to the node grid.
  defp detail_node_counts(%{"states" => states}) when is_map(states) do
    order = ["running", "pending", "succeeded", "failed", "skipped", "upstream_failed", "stranded", "cancelled"]

    parts =
      for state <- order, count = Map.get(states, state, 0), count > 0 do
        "#{count} #{state}"
      end

    case parts do
      [] -> "0 nodes"
      _ -> Enum.join(parts, " - ")
    end
  end

  defp detail_node_counts(_), do: "0 nodes"

  # Render a placement map as a human-readable label. When declared and
  # effective differ (a fallback occurred), both are shown so the operator
  # can see exactly what happened. Nil placement means placement was not
  # recorded (e.g. a local-only run or a run predating the placement stamp).
  defp placement_label(nil), do: "-"

  defp placement_label(%{"declared" => declared, "effective" => effective})
       when declared == effective or is_nil(effective) do
    declared || "-"
  end

  defp placement_label(%{"declared" => declared, "effective" => effective}) do
    "#{declared} (fallback #{effective})"
  end

  defp placement_label(_), do: "-"

  defp cost_label(nil), do: "-"
  defp cost_label(usd) when is_number(usd), do: "$" <> :erlang.float_to_binary(usd / 1, decimals: 4)

  defp node_cost(%{"attempts" => attempts}) when is_list(attempts) do
    usd =
      for %{"cost" => %{"usd" => usd}} <- attempts, is_number(usd), reduce: nil do
        acc -> (acc || 0) + usd
      end

    cost_label(usd)
  end

  defp node_cost(_), do: "-"

  defp engine_label(%{"engine" => engine}) when is_binary(engine), do: " - " <> engine
  defp engine_label(_), do: ""

  # Delegate to the shared formatter in View so the form dropdown and the
  # summary card always show the same label for a given trigger.
  defp trigger_label(trigger), do: View.trigger_label(trigger)

  defp relative_time(nil), do: ""

  defp relative_time(iso) when is_binary(iso) do
    case DateTime.from_iso8601(iso) do
      {:ok, dt, _} ->
        s = DateTime.diff(DateTime.utc_now(), dt, :second)

        cond do
          s < 60 -> "#{s}s ago"
          s < 3600 -> "#{div(s, 60)}m ago"
          true -> "#{div(s, 3600)}h ago"
        end

      _ ->
        iso
    end
  end
end
