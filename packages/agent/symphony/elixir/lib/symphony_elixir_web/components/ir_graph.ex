defmodule SymphonyElixirWeb.Components.IRGraph do
  @moduledoc """
  Server-rendered SVG IR-graph component.

  Accepts a list of node maps as produced by `IR.View.render_node/1` and
  lays them out in a left-to-right layered DAG using a longest-path-from-roots
  algorithm. Edges are cubic bezier curves. No JavaScript library is required;
  the SVG is emitted directly from the server on every live update.

  State classes on each `<g class="gnode ...">` element match the `.gnode.*`
  CSS rules in the application layout so colors track live node state without
  a client-side refresh.

  When a trigger label is provided, a synthetic trigger node (class `gtrigger`)
  is prepended as the entry that feeds all root nodes. The trigger node is not
  part of the IR; it only appears in the visual graph.

  The placement map (with `"declared"` and `"effective"` keys) is forwarded
  to agent nodes so the graph can show a fallback label when the effective
  location differs from what was declared.
  """

  use Phoenix.Component

  # Layout spacing. Node boxes are sized to their content (see node_width/1
  # and node_height/1) so a long cron schedule or script path never spills
  # past the rect; @min_node_w keeps short graphs from looking cramped.
  @min_node_w 180
  @h_gap 80
  @v_gap 20
  @pad_x 30
  @pad_y 22

  # Text geometry, shared between the SVG template and the box-sizing helpers.
  # Monospace advance is about 0.6em; the per-char widths are biased slightly
  # wide so a glyph never crosses the border. @label_char_w sizes the bold
  # 12px label line, @detail_char_w the 10px id and detail lines.
  @label_char_w 7.6
  @detail_char_w 6.4
  @text_left 10
  @text_right 14
  @id_y 29
  @detail_top 44
  @detail_step 13
  @bottom_pad 13

  @doc """
  Render an inline SVG graph for a list of IR nodes.

  `nodes` is the `"nodes"` list from `IR.View.detail/1`: each element is a
  string-keyed map with at least `"id"`, `"kind"`, `"state"`, `"deps"`, and
  (for agent nodes) `"envelope"` and `"label"`.

  `trigger` is the human-readable trigger string from `IR.View.detail/1`
  (e.g. `"cron 30 * * * *"`, `"manual"`, `"linear: [sym] implement"`). When
  provided, a distinct trigger node is drawn as the entry feeding all roots.

  `placement` is the placement map from `IR.View.detail/1` (keys
  `"declared"` and `"effective"`), forwarded to agent nodes so a fallback can
  be shown when effective differs from declared.

  `base_path` is unused in rendering but kept as an attribute to allow
  future click-through links without a breaking interface change.
  """
  attr(:nodes, :list, required: true)
  attr(:trigger, :string, default: nil)
  attr(:placement, :map, default: nil)
  attr(:base_path, :string, default: "/ir")

  @spec graph(map()) :: Phoenix.LiveView.Rendered.t()
  def graph(assigns) do
    assigns = assign(assigns, :layout, layout(assigns.nodes, assigns.trigger, assigns.placement))

    ~H"""
    <svg
      class="graph"
      viewBox={@layout.viewbox}
      style={"max-width: #{@layout.natural_width}px"}
      role="img"
      aria-label="IR graph"
    >
      <defs>
        <marker id="arrow" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
          <path class="garrow" d="M0,0 L6,3 L0,6 Z" />
        </marker>
      </defs>
      <%= for edge <- @layout.edges do %>
        <path class="gedge" d={edge.d} marker-end="url(#arrow)" />
      <% end %>
      <%= for n <- @layout.nodes do %>
        <g class={"gnode " <> n.state_class} transform={"translate(#{n.x},#{n.y})"}>
          <rect width={@layout.node_w} height={@layout.node_h} />
          <text x="10" y={if n.is_trigger, do: div(@layout.node_h, 2) + 4, else: 16} class="gnode-label">{n.label}</text>
          <text :if={not n.is_trigger} x="10" y="29" class="gnode-id" opacity=".55">{n.id}</text>
          <%= for {line, idx} <- Enum.with_index(n.detail_lines) do %>
            <text x="10" y={44 + idx * 13} class="gnode-detail" opacity=".7">{line}</text>
          <% end %>
        </g>
      <% end %>
    </svg>
    """
  end

  @doc """
  Pure layout computation: assigns each node a layer by longest-path-from-roots
  over `deps`, orders nodes within a layer by their first appearance in the input
  list, and returns pixel coordinates plus bezier edge paths.

  When `trigger` is a non-nil string, a synthetic trigger node is prepended
  in layer -1 (rendered as layer 0 with all real nodes shifted right), and
  edges are drawn from it to every root node. The trigger node carries the
  class `gtrigger`.

  Returns a map with:
  - `viewbox` - the SVG `viewBox` attribute string
  - `natural_width` - the numeric pixel width so the caller can cap `max-width`
  - `node_w` / `node_h` - the content-fitted box dimensions the template draws
  - `nodes` - list of maps with `:id`, `:x`, `:y`, `:state_class`, `:label`,
    `:detail_lines`, `:is_trigger`
  - `edges` - list of maps with `:d` (SVG path data string)

  The function is public so it can be unit-tested independently of the
  LiveView/component machinery.
  """
  @spec layout([map()], String.t() | nil, map() | nil) :: %{
          viewbox: String.t(),
          natural_width: integer(),
          node_w: integer(),
          node_h: integer(),
          nodes: [map()],
          edges: [map()]
        }
  def layout(nodes, trigger \\ nil, placement \\ nil)

  @spec layout([map()], String.t() | nil, map() | nil) :: %{
          viewbox: String.t(),
          natural_width: integer(),
          node_w: integer(),
          node_h: integer(),
          nodes: [map()],
          edges: [map()]
        }
  def layout([], nil, _placement) do
    %{viewbox: "0 0 200 80", natural_width: 200, node_w: @min_node_w, node_h: 80, nodes: [], edges: []}
  end

  @spec layout([map()], String.t() | nil, map() | nil) :: %{
          viewbox: String.t(),
          natural_width: integer(),
          node_w: integer(),
          node_h: integer(),
          nodes: [map()],
          edges: [map()]
        }
  def layout([], trigger, _placement) when is_binary(trigger) do
    sizing = [%{id: nil, label: trigger, detail_lines: []}]
    node_w = node_width(sizing)
    node_h = node_height(sizing)
    width = @pad_x + node_w + @pad_x
    height = @pad_y + node_h + @pad_y

    trigger_node = %{
      id: "__trigger__",
      x: @pad_x,
      y: @pad_y,
      state_class: "gtrigger",
      label: trigger,
      detail_lines: [],
      is_trigger: true
    }

    %{
      viewbox: "0 0 #{width} #{height}",
      natural_width: width,
      node_w: node_w,
      node_h: node_h,
      nodes: [trigger_node],
      edges: []
    }
  end

  @spec layout([map()], String.t() | nil, map() | nil) :: %{
          viewbox: String.t(),
          natural_width: integer(),
          node_w: integer(),
          node_h: integer(),
          nodes: [map()],
          edges: [map()]
        }
  def layout(nodes, trigger, placement) when is_list(nodes) do
    # Build a node-id to deps map and compute layer assignments.
    deps_map = Map.new(nodes, fn n -> {n["id"], n["deps"] || []} end)
    layers = assign_layers(deps_map)

    # When a trigger is provided, shift all real node layers by 1 to make
    # room for the synthetic trigger node at layer 0.
    layers =
      if trigger do
        Map.new(layers, fn {id, layer} -> {id, layer + 1} end)
      else
        layers
      end

    # Group node ids by layer, preserving original list order within a layer.
    id_order = nodes |> Enum.with_index() |> Map.new(fn {n, i} -> {n["id"], i} end)

    layer_groups =
      layers
      |> Enum.group_by(fn {_id, layer} -> layer end, fn {id, _layer} -> id end)
      |> Map.new(fn {layer, ids} -> {layer, Enum.sort_by(ids, &Map.get(id_order, &1, 0))} end)

    max_layer = layers |> Map.values() |> Enum.max(fn -> 0 end)

    # When a trigger is present, layer 0 holds only the synthetic trigger node
    # (one row). For real nodes, the tallest layer among layers >= 1 determines
    # vertical height.
    real_max_per_layer =
      layer_groups
      |> Map.drop([0])
      |> Map.values()
      |> Enum.map(&length/1)
      |> Enum.max(fn -> 1 end)

    max_per_layer =
      if trigger do
        max(real_max_per_layer, 1)
      else
        layer_groups |> Map.values() |> Enum.map(&length/1) |> Enum.max(fn -> 1 end)
      end

    # Compute pixel coordinates for each real node.
    node_index = Map.new(nodes, fn n -> {n["id"], n} end)

    # Pre-compute each node's render data (label, id, detail lines) so the box
    # can be sized to its content before positioning. A fixed rect width would
    # be overflowed by a long label such as a verbose cron schedule.
    render_by_id =
      Map.new(nodes, fn raw ->
        {raw["id"],
         %{
           id: raw["id"],
           state_class: state_class(raw),
           label: primary_label(raw),
           detail_lines: detail_lines(raw, placement)
         }}
      end)

    sizing =
      Map.values(render_by_id) ++
        if(trigger, do: [%{id: nil, label: trigger, detail_lines: []}], else: [])

    node_w = node_width(sizing)
    node_h = node_height(sizing)

    positioned =
      for {id, raw} <- node_index do
        layer = Map.get(layers, id, 0)
        pos_in_layer = Enum.find_index(layer_groups[layer], &(&1 == id)) || 0
        total_in_layer = length(layer_groups[layer])

        x = @pad_x + layer * (node_w + @h_gap)
        # Center nodes vertically within their layer relative to the tallest layer.
        offset_y = div((max_per_layer - total_in_layer) * (node_h + @v_gap), 2)
        y = @pad_y + pos_in_layer * (node_h + @v_gap) + offset_y

        {id, %{x: x, y: y, raw: raw}}
      end
      |> Map.new()

    # Build edge paths: one bezier per dep edge between real nodes.
    edges =
      for {id, %{x: tx, y: ty}} <- positioned,
          dep_id <- node_index[id]["deps"] || [],
          is_binary(dep_id),
          Map.has_key?(positioned, dep_id) do
        %{x: sx, y: sy} = positioned[dep_id]
        bezier_edge(sx, sy, tx, ty, node_w, node_h)
      end

    # Find root real nodes (those with no real deps) to connect from trigger.
    root_ids =
      if trigger do
        Enum.filter(nodes, fn n ->
          known_deps = Enum.filter(n["deps"] || [], &Map.has_key?(node_index, &1))
          known_deps == []
        end)
        |> Enum.map(& &1["id"])
      else
        []
      end

    # Synthetic trigger node sits in column 0; real nodes start at column 1.
    trigger_x = @pad_x
    trigger_y = @pad_y + div((max_per_layer - 1) * (node_h + @v_gap), 2)

    trigger_edges =
      for root_id <- root_ids,
          Map.has_key?(positioned, root_id) do
        %{x: tx, y: ty} = positioned[root_id]
        bezier_edge(trigger_x, trigger_y, tx, ty, node_w, node_h)
      end

    all_edges = edges ++ trigger_edges

    # Build the layout node list.
    layout_nodes =
      Enum.map(nodes, fn raw ->
        id = raw["id"]
        %{x: x, y: y} = positioned[id]
        render = render_by_id[id]

        %{
          id: id,
          x: x,
          y: y,
          state_class: render.state_class,
          label: render.label,
          detail_lines: render.detail_lines,
          is_trigger: false
        }
      end)

    # Prepend the trigger node when present.
    all_nodes =
      if trigger do
        trigger_node = %{
          id: "__trigger__",
          x: trigger_x,
          y: trigger_y,
          state_class: "gtrigger",
          label: trigger,
          detail_lines: [],
          is_trigger: true
        }

        [trigger_node | layout_nodes]
      else
        layout_nodes
      end

    # Size the viewBox to fit all content.
    width = @pad_x + (max_layer + 1) * (node_w + @h_gap) - @h_gap + @pad_x
    height = @pad_y + max_per_layer * (node_h + @v_gap) - @v_gap + @pad_y

    %{
      viewbox: "0 0 #{width} #{height}",
      natural_width: width,
      node_w: node_w,
      node_h: node_h,
      nodes: all_nodes,
      edges: all_edges
    }
  end

  # Emit a cubic bezier edge from the right side of the source node to the
  # left side of the target node. Coordinates are for the node's top-left corner.
  defp bezier_edge(sx, sy, tx, ty, node_w, node_h) do
    x1 = sx + node_w
    y1 = sy + div(node_h, 2)
    x2 = tx
    y2 = ty + div(node_h, 2)
    mid_x = div(x1 + x2, 2)
    %{d: "M#{x1},#{y1} C#{mid_x},#{y1} #{mid_x},#{y2} #{x2},#{y2}"}
  end

  # Box width fits the widest rendered line so a long label never crosses the
  # border. The label uses the bold 12px font; the id and detail lines the
  # 10px font. @min_node_w keeps short graphs from looking cramped.
  defp node_width(renders) do
    widest =
      renders
      |> Enum.flat_map(&line_pixel_widths/1)
      |> Enum.max(fn -> 0.0 end)

    max(@min_node_w, ceil(widest) + @text_left + @text_right)
  end

  defp line_pixel_widths(render) do
    label = text_px(render.label, @label_char_w)
    id = text_px(render[:id], @detail_char_w)
    details = Enum.map(render.detail_lines, &text_px(&1, @detail_char_w))
    [label, id | details]
  end

  defp text_px(nil, _char_w), do: 0.0
  defp text_px(text, char_w) when is_binary(text), do: String.length(text) * char_w

  # Box height fits the tallest node: the label and id rows plus the deepest
  # detail block in the graph (agent nodes carry up to four envelope lines).
  defp node_height(renders) do
    max_detail = renders |> Enum.map(&length(&1.detail_lines)) |> Enum.max(fn -> 0 end)

    bottom =
      if max_detail > 0 do
        @detail_top + (max_detail - 1) * @detail_step
      else
        @id_y
      end

    bottom + @bottom_pad
  end

  # Assigns each node a layer by the longest path from any root (a node with
  # no incoming deps). Nodes with no deps are in layer 0; a node's layer is
  # one more than the maximum layer of its dependencies. Returns a map of
  # node_id => layer_number.
  defp assign_layers(deps_map) when is_map(deps_map) do
    # Compute layer for each node via memoized recursion. Uses a plain reduce
    # over a stable topological ordering to avoid stack issues on deep graphs.
    Enum.reduce(Map.keys(deps_map), %{}, fn id, acc ->
      compute_layer(id, deps_map, acc)
    end)
  end

  defp compute_layer(id, deps_map, memo) do
    case Map.fetch(memo, id) do
      {:ok, _layer} ->
        memo

      :error ->
        deps = Map.get(deps_map, id, [])
        # Only consider deps that exist in the graph; skip dangling edges.
        known_deps = Enum.filter(deps, &Map.has_key?(deps_map, &1))

        memo =
          Enum.reduce(known_deps, memo, fn dep_id, acc ->
            compute_layer(dep_id, deps_map, acc)
          end)

        layer =
          case known_deps do
            [] ->
              0

            _ ->
              known_deps
              |> Enum.map(&Map.get(memo, &1, 0))
              |> Enum.max()
              |> Kernel.+(1)
          end

        Map.put(memo, id, layer)
    end
  end

  # Map a node's state (and kind for gate) to a CSS class string.
  # Gate nodes get an additional `gate` class so the dashed border rule fires.
  defp state_class(%{"kind" => "gate", "state" => state}), do: "gate " <> normalize_state(state)
  defp state_class(%{"state" => state}), do: normalize_state(state)
  defp state_class(_), do: "pending"

  # The CSS rules cover succeeded/running/pending/failed/skipped. Unknown or
  # terminal-adjacent states (upstream_failed, stranded, cancelled) fall back
  # to "pending" visually so the SVG never references an undefined class.
  defp normalize_state("succeeded"), do: "succeeded"
  defp normalize_state("running"), do: "running"
  defp normalize_state("pending"), do: "pending"
  defp normalize_state("failed"), do: "failed"
  defp normalize_state("skipped"), do: "skipped"
  defp normalize_state(_), do: "pending"

  # Primary label: the skill name for agent nodes, script path for exec nodes,
  # or the kind for other nodes. Falls back to the node id when no label field
  # is present (for nodes rendered from older view shapes).
  defp primary_label(%{"label" => label}) when is_binary(label) and label != "", do: label
  defp primary_label(%{"id" => id}), do: id

  # Build the detail lines shown below the primary label. Agent nodes show
  # engine/model, effort, permissions, and location (with fallback notation
  # when the envelope location differs from the node id's effective location).
  # Exec/subrun/gate nodes show only their kind target.
  defp detail_lines(%{"kind" => "agent", "envelope" => env}, placement) when is_map(env) do
    engine_model =
      case {env["engine"], env["model"]} do
        {e, m} when is_binary(e) and is_binary(m) -> "#{e} #{m}"
        {e, nil} when is_binary(e) -> e
        _ -> nil
      end

    effort = env["effort"]
    permissions = env["permissions"]
    location = location_line(env["location"], placement)

    [engine_model, effort, permissions, location]
    |> Enum.filter(&is_binary/1)
    |> Enum.reject(&(&1 == ""))
  end

  defp detail_lines(%{"kind" => "gate"}, _placement), do: ["gate"]
  defp detail_lines(%{"kind" => "map_fanout"}, _placement), do: ["map_fanout"]
  defp detail_lines(%{"kind" => kind}, _placement) when is_binary(kind), do: [kind]
  defp detail_lines(_, _placement), do: []

  # Annotate a placement fallback on the location line. When the run's
  # effective placement type differs from the node's declared location (e.g.
  # declared `ixvm` but the host could not start a guest so it ran on the
  # host), the line reads `ixvm (fallback host)`. The declared location may be
  # a typed string like `host:hil-compute-2`, so only the type before `:` is
  # compared. With no placement (a `/workflows` preview before any run) the
  # declared location is shown as-is.
  defp location_line(location, %{"effective" => effective})
       when is_binary(location) and is_binary(effective) do
    declared_type = location |> String.split(":") |> hd()

    if declared_type == effective do
      location
    else
      "#{location} (fallback #{effective})"
    end
  end

  defp location_line(location, _placement), do: location
end
