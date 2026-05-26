<script lang="ts">
  import type { Node, Edge } from '@xyflow/svelte';
  import DiagramFrame from './DiagramFrame.svelte';

  const nodes: Node[] = [
    {
      id: 'a',
      type: 'box',
      position: { x: 0, y: 0 },
      data: { kicker: 'browser', label: 'teammate A', sublabel: 'name, focus, draft', kind: 'agent' }
    },
    {
      id: 'b',
      type: 'box',
      position: { x: 0, y: 110 },
      data: { kicker: 'browser', label: 'teammate B', sublabel: 'codex: editing', kind: 'agent' }
    },
    {
      id: 'c',
      type: 'box',
      position: { x: 0, y: 220 },
      data: { kicker: 'browser', label: 'teammate C', sublabel: 'codex: reviewing', kind: 'agent' }
    },
    {
      id: 'ws',
      type: 'box',
      position: { x: 280, y: 60 },
      data: { kicker: 'axum /ws', label: 'websocket gateway', kind: 'proc' }
    },
    {
      id: 'state',
      type: 'box',
      position: { x: 280, y: 170 },
      data: { kicker: 'tokio::Mutex', label: 'LoroDoc + participants', sublabel: 'apply + stamp lastSeen', kind: 'proc' }
    },
    {
      id: 'bus',
      type: 'box',
      position: { x: 540, y: 110 },
      data: { kicker: 'broadcast::Sender', label: 'snapshot fanout', kind: 'artifact' }
    }
  ];

  const edges: Edge[] = [
    { id: 'a-ws', source: 'a', target: 'ws', label: 'presence' },
    { id: 'b-ws', source: 'b', target: 'ws', label: 'codex' },
    { id: 'c-ws', source: 'c', target: 'ws', label: 'focus' },
    { id: 'ws-state', source: 'ws', target: 'state' },
    { id: 'state-bus', source: 'state', target: 'bus' },
    { id: 'bus-a', source: 'bus', target: 'a', sourceHandle: 't-out', targetHandle: 't-in', label: 'snapshot' },
    { id: 'bus-b', source: 'bus', target: 'b', sourceHandle: 'l-out', targetHandle: 'r-in', label: 'snapshot' },
    { id: 'bus-c', source: 'bus', target: 'c', sourceHandle: 'b-out', targetHandle: 'b-in', label: 'snapshot' }
  ];
</script>

<DiagramFrame {nodes} {edges} height={340} caption="Each client posts intent; every apply rebroadcasts a fresh snapshot to every connected teammate." />
