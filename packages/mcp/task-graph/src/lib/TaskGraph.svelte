<script lang="ts">
  import { onMount } from 'svelte';
  import ForceGraph from 'force-graph';
  import { forceCollide } from 'd3-force';
  import {
    statusOf,
    STATUS_META,
    CATEGORY_COLORS,
    type Task,
  } from './types';

  type ColorMode = 'status' | 'category';

  interface GNode {
    id: string;
    task: Task;
    val: number;
    x?: number;
    y?: number;
  }
  interface GLink {
    source: string | GNode;
    target: string | GNode;
  }

  let {
    tasks,
    colorMode = 'status',
    dagMode = 'td',
    selected = $bindable(null),
  }: {
    tasks: Task[];
    colorMode?: ColorMode;
    dagMode?: 'td' | 'lr' | null;
    selected?: Task | null;
  } = $props();

  let container: HTMLDivElement;
  let graph: ForceGraph<GNode, GLink> | null = null;

  const byId = $derived(new Map(tasks.map((t) => [t.id, t])));

  function nodeColor(t: Task): string {
    return colorMode === 'status'
      ? STATUS_META[statusOf(t, byId)].color
      : CATEGORY_COLORS[t.category];
  }

  // The node in focus drives the hierarchy fade: hover takes priority, else the
  // selected node.
  let hoverId = $state<string | null>(null);
  const focusId = $derived(hoverId ?? selected?.id ?? null);

  const adjacency = $derived.by(() => {
    const m = new Map<string, Set<string>>();
    for (const t of tasks) m.set(t.id, new Set());
    for (const t of tasks) {
      for (const d of t.deps) {
        m.get(t.id)!.add(d);
        m.get(d)!.add(t.id);
      }
    }
    return m;
  });

  // BFS hop-distance from the focused node. null when nothing is focused (so the
  // whole graph stays at full brightness).
  const distance = $derived.by<Map<string, number> | null>(() => {
    if (focusId == null) return null;
    const dist = new Map<string, number>([[focusId, 0]]);
    const queue = [focusId];
    for (let i = 0; i < queue.length; i++) {
      const cur = queue[i];
      const d = dist.get(cur)!;
      for (const nb of adjacency.get(cur) ?? []) {
        if (!dist.has(nb)) {
          dist.set(nb, d + 1);
          queue.push(nb);
        }
      }
    }
    return dist;
  });

  // Each ring is ~55% as bright as the one before it; unreachable nodes are barely
  // visible so the connected hierarchy pops out.
  const DECAY = 0.55;
  function alphaFor(id: string): number {
    if (!distance) return 1;
    const d = distance.get(id);
    if (d === undefined) return 0.05;
    return Math.max(0.1, DECAY ** d);
  }

  function buildData() {
    const nodes: GNode[] = tasks.map((t) => ({
      id: t.id,
      task: t,
      val: t.estimate,
    }));
    const links: GLink[] = [];
    for (const t of tasks) {
      for (const d of t.deps) links.push({ source: d, target: t.id });
    }
    return { nodes, links };
  }

  function idOf(end: string | GNode): string {
    return typeof end === 'object' ? end.id : end;
  }

  // Drawn radius in graph units; reused by the collision force so circles never
  // overlap regardless of their estimate-driven size.
  function radius(n: GNode): number {
    return Math.max(2, Math.sqrt(Math.max(1.5, n.val)) * 1.8);
  }

  onMount(() => {
    graph = new ForceGraph<GNode, GLink>(container)
      .backgroundColor('#0d1117')
      // Repaint every frame so hover/selection highlighting and color toggles show
      // instantly. The alternative -- poking a setter to force a redraw -- restarts
      // the layout engine and makes the whole graph jump on every hover.
      .autoPauseRedraw(false)
      // We drive zoom ourselves so trackpad two-finger scroll pans (see wheel handler)
      .enableZoomInteraction(false)
      .nodeId('id')
      .nodeVal((n: GNode) => Math.max(1.5, n.val))
      .nodeRelSize(4)
      .linkColor((l: GLink) => {
        if (!distance) return 'rgba(139,148,158,0.22)';
        const ds = distance.get(idOf(l.source));
        const dt = distance.get(idOf(l.target));
        if (ds === undefined || dt === undefined) return 'rgba(139,148,158,0.04)';
        const a = Math.max(0.1, DECAY ** Math.max(ds, dt));
        return Math.min(ds, dt) === 0
          ? `rgba(88,166,255,${Math.max(0.55, a)})`
          : `rgba(139,148,158,${a * 0.7})`;
      })
      .linkWidth((l: GLink) => {
        if (!distance) return 0.6;
        const ds = distance.get(idOf(l.source));
        const dt = distance.get(idOf(l.target));
        if (ds === undefined || dt === undefined) return 0.4;
        return Math.min(ds, dt) === 0 ? 2 : 0.7;
      })
      .linkDirectionalArrowLength(3.5)
      .linkDirectionalArrowRelPos(1)
      .nodeLabel((n: GNode) => {
        const s = statusOf(n.task, byId);
        return `<b>${n.task.title}</b><br/>${n.id} · ${n.task.category} · ${STATUS_META[s].label}`;
      })
      .onNodeHover((n: GNode | null) => {
        hoverId = n?.id ?? null;
        container.style.cursor = n ? 'pointer' : '';
      })
      .onNodeClick((n: GNode) => {
        selected = n.task;
        graph?.centerAt(n.x, n.y, 600);
        graph?.zoom(2.5, 600);
      })
      .onBackgroundClick(() => {
        selected = null;
      })
      .nodeCanvasObjectMode(() => 'replace')
      .nodeCanvasObject((n, ctx, globalScale) => {
        const r = radius(n);
        const focused = focusId === n.id;
        const a = alphaFor(n.id);

        ctx.beginPath();
        ctx.arc(n.x!, n.y!, r, 0, 2 * Math.PI);
        ctx.fillStyle = withAlpha(nodeColor(n.task), a);
        ctx.fill();

        if (focused) {
          ctx.lineWidth = 1.5;
          ctx.strokeStyle = '#e6edf3';
          ctx.stroke();
        }

        // Label the focused node, its direct neighbours, or everything when zoomed in.
        const near = distance ? (distance.get(n.id) ?? 99) <= 1 : false;
        if (globalScale > 1.6 || focused || near) {
          const fontSize = Math.min(5, 11 / globalScale);
          ctx.font = `${fontSize}px ui-sans-serif, system-ui, sans-serif`;
          ctx.textAlign = 'center';
          ctx.textBaseline = 'top';
          ctx.fillStyle = `rgba(230,237,243,${Math.max(0.25, a)})`;
          ctx.fillText(n.task.title, n.x!, n.y! + r + 1);
        }
      });

    if (dagMode) graph.dagMode(dagMode).dagLevelDistance(70);
    graph.graphData(buildData()).d3VelocityDecay(0.3);

    // Spread the graph out: stronger repulsion, longer links, and a hard collision
    // radius so node circles (and their labels) stop overlapping.
    graph.d3Force('charge')?.strength(-220);
    graph.d3Force('link')?.distance(48);
    graph.d3Force(
      'collide',
      forceCollide<GNode>((n) => radius(n) + 6).strength(1),
    );
    graph.d3ReheatSimulation();

    // Trackpad UX: two-finger scroll pans, pinch (ctrl+wheel) zooms toward cursor.
    function onWheel(e: WheelEvent) {
      if (!graph) return;
      e.preventDefault();
      const k = graph.zoom();

      if (e.ctrlKey) {
        const rect = container.getBoundingClientRect();
        const sx = e.clientX - rect.left;
        const sy = e.clientY - rect.top;
        const before = graph.screen2GraphCoords(sx, sy);
        const nextK = clamp(k * Math.exp(-e.deltaY * 0.01), 0.05, 12);
        graph.zoom(nextK);
        const after = graph.screen2GraphCoords(sx, sy);
        const c = graph.centerAt();
        graph.centerAt(c.x + (before.x - after.x), c.y + (before.y - after.y));
      } else {
        const c = graph.centerAt();
        graph.centerAt(c.x + e.deltaX / k, c.y + e.deltaY / k);
      }
    }
    container.addEventListener('wheel', onWheel, { passive: false });

    const ro = new ResizeObserver(() => {
      graph?.width(container.clientWidth).height(container.clientHeight);
    });
    ro.observe(container);
    graph.width(container.clientWidth).height(container.clientHeight);
    setTimeout(() => graph?.zoomToFit(500, 60), 400);

    return () => {
      container.removeEventListener('wheel', onWheel);
      ro.disconnect();
      graph?._destructor?.();
      graph = null;
    };
  });

  // Only layout changes need a setter; everything else is read live each frame.
  $effect(() => {
    if (!graph) return;
    graph.dagMode(dagMode ?? null);
    if (dagMode) graph.dagLevelDistance(70);
    graph.d3ReheatSimulation();
    setTimeout(() => graph?.zoomToFit(500, 60), 300);
  });

  function clamp(v: number, lo: number, hi: number): number {
    return Math.min(hi, Math.max(lo, v));
  }

  function withAlpha(hex: string, a: number): string {
    const h = hex.replace('#', '');
    const r = parseInt(h.slice(0, 2), 16);
    const g = parseInt(h.slice(2, 4), 16);
    const b = parseInt(h.slice(4, 6), 16);
    return `rgba(${r},${g},${b},${a})`;
  }

  export function focus(task: Task) {
    selected = task;
    const node = graph
      ?.graphData()
      .nodes.find((n) => n.id === task.id);
    if (node) {
      graph?.centerAt(node.x, node.y, 600);
      graph?.zoom(2.5, 600);
    }
  }

  export function resetView() {
    selected = null;
    graph?.zoomToFit(500, 60);
  }
</script>

<div class="graph" bind:this={container}></div>

<style>
  .graph {
    width: 100%;
    height: 100%;
  }
</style>
