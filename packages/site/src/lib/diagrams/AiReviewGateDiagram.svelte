<script lang="ts">
  import type { Node, Edge } from '@xyflow/svelte';
  import DiagramFrame from './DiagramFrame.svelte';

  const nodes: Node[] = [
    {
      id: 'pr',
      type: 'box',
      position: { x: 0, y: 100 },
      data: { kicker: 'github event', label: 'pull request', sublabel: 'opened / sync', kind: 'agent' }
    },
    {
      id: 'trust',
      type: 'box',
      position: { x: 190, y: 100 },
      data: { label: 'trust check', sublabel: 'same-repo? fork? bot?', kind: 'gate' }
    },
    {
      id: 'codex',
      type: 'box',
      position: { x: 400, y: 20 },
      data: { kicker: 'openai/codex-action', label: 'secret-backed reviewer', sublabel: 'xhigh effort', kind: 'proc' }
    },
    {
      id: 'wait',
      type: 'box',
      position: { x: 400, y: 180 },
      data: { label: 'await maintainer approval', sublabel: 'no secrets, no run', kind: 'gate' }
    },
    {
      id: 'json',
      type: 'box',
      position: { x: 640, y: 20 },
      data: { kicker: 'json schema', label: 'findings + verdict', sublabel: 'suggested_replacement', kind: 'artifact' }
    },
    {
      id: 'gate',
      type: 'box',
      position: { x: 870, y: 100 },
      data: { label: 'ai review approved', sublabel: 'branch protection', kind: 'gate' }
    },
    {
      id: 'comments',
      type: 'box',
      position: { x: 640, y: 180 },
      data: { label: 'github review suggestions', sublabel: 'one-click apply', kind: 'artifact' }
    }
  ];

  const edges: Edge[] = [
    { id: 'pr-trust', source: 'pr', target: 'trust' },
    { id: 'trust-codex', source: 'trust', target: 'codex', label: 'same-repo' },
    { id: 'trust-wait', source: 'trust', target: 'wait', label: 'fork / dependabot' },
    { id: 'codex-json', source: 'codex', target: 'json' },
    { id: 'json-gate', source: 'json', target: 'gate' },
    { id: 'json-comments', source: 'json', target: 'comments' },
    { id: 'wait-gate', source: 'wait', target: 'gate', label: 'after approval' }
  ];
</script>

<DiagramFrame {nodes} {edges} height={320} caption="Same-repo PRs run Codex; fork PRs wait for a trusted approval, both ending at the same gate." />
