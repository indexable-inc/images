<script lang="ts">
  import type { Node, Edge } from '@xyflow/svelte';
  import DiagramFrame from './DiagramFrame.svelte';

  const nodes: Node[] = [
    {
      id: 'agent',
      type: 'box',
      position: { x: 0, y: 80 },
      data: { kicker: 'mcp client', label: 'Coding agent', sublabel: 'python_exec / python_eval', kind: 'agent' }
    },
    {
      id: 'mcp',
      type: 'box',
      position: { x: 220, y: 80 },
      data: { kicker: 'nix run .#mcp', label: 'ix-mcp server', sublabel: 'stdio router', kind: 'proc' }
    },
    {
      id: 'worker',
      type: 'box',
      position: { x: 450, y: 30 },
      data: { kicker: 'subprocess', label: 'python worker', sublabel: 'per-session venv', kind: 'proc' }
    },
    {
      id: 'globals',
      type: 'box',
      position: { x: 450, y: 140 },
      data: { label: 'session globals', sublabel: 'df, slow, imports', kind: 'artifact' }
    },
    {
      id: 'data',
      type: 'box',
      position: { x: 670, y: 80 },
      data: { kicker: '.ix/run/latest', label: 'lines.jsonl', sublabel: 'recorded by run', kind: 'artifact' }
    }
  ];

  const edges: Edge[] = [
    { id: 'agent-mcp', source: 'agent', target: 'mcp', label: 'tool call' },
    { id: 'mcp-worker', source: 'mcp', target: 'worker' },
    { id: 'worker-globals', source: 'worker', target: 'globals', label: 'persist' },
    { id: 'globals-worker', source: 'globals', target: 'worker', sourceHandle: 't-out', targetHandle: 'b-in', label: 'reuse' },
    { id: 'worker-data', source: 'worker', target: 'data', label: 'read_ndjson' },
    { id: 'mcp-agent', source: 'mcp', target: 'agent', sourceHandle: 'b-out', targetHandle: 'b-in', label: 'stdout / result' }
  ];
</script>

<DiagramFrame {nodes} {edges} height={300} caption="One stdio session, one persistent Python interpreter, many tool calls." />
