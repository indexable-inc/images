<script lang="ts">
  import type { Node, Edge } from '@xyflow/svelte';
  import DiagramFrame from './DiagramFrame.svelte';

  const nodes: Node[] = [
    {
      id: 'cmd',
      type: 'box',
      position: { x: 0, y: 110 },
      data: { kicker: 'argv', label: 'cargo test -p my-crate', kind: 'agent' }
    },
    {
      id: 'run',
      type: 'box',
      position: { x: 200, y: 110 },
      data: { kicker: 'nix run .#run', label: 'PTY recorder', sublabel: 'head/tail to stdout', kind: 'proc' }
    },
    {
      id: 'log',
      type: 'box',
      position: { x: 430, y: 0 },
      data: { label: 'output.log', sublabel: 'tail -f live', kind: 'artifact' }
    },
    {
      id: 'lines',
      type: 'box',
      position: { x: 430, y: 75 },
      data: { label: 'lines.jsonl', sublabel: 'per-line timing', kind: 'artifact' }
    },
    {
      id: 'cast',
      type: 'box',
      position: { x: 430, y: 150 },
      data: { label: 'session.cast', sublabel: 'asciinema + replay', kind: 'artifact' }
    },
    {
      id: 'summary',
      type: 'box',
      position: { x: 430, y: 225 },
      data: { label: 'summary.json', sublabel: 'status, duration', kind: 'artifact' }
    },
    {
      id: 'polars',
      type: 'box',
      position: { x: 670, y: 75 },
      data: { kicker: 'pl.read_ndjson', label: 'polars query', sublabel: 'slowest lines / phases', kind: 'proc' }
    }
  ];

  const edges: Edge[] = [
    { id: 'cmd-run', source: 'cmd', target: 'run' },
    { id: 'run-log', source: 'run', target: 'log' },
    { id: 'run-lines', source: 'run', target: 'lines' },
    { id: 'run-cast', source: 'run', target: 'cast' },
    { id: 'run-summary', source: 'run', target: 'summary' },
    { id: 'lines-polars', source: 'lines', target: 'polars' }
  ];
</script>

<DiagramFrame {nodes} {edges} height={320} caption="One PTY in, one structured artifact set out, ready for polars." />
