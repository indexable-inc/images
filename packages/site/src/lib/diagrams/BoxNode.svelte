<script lang="ts">
  import { Handle, Position, type Node, type NodeProps } from '@xyflow/svelte';

  type BoxData = {
    kicker?: string;
    label: string;
    sublabel?: string;
    kind?: 'default' | 'proc' | 'artifact' | 'agent' | 'gate';
  };

  type BoxNodeType = Node<BoxData, 'box'>;

  const { data }: NodeProps<BoxNodeType> = $props();
</script>

<div class="box" data-kind={data.kind ?? 'default'}>
  {#if data.kicker}<div class="kicker">{data.kicker}</div>{/if}
  <div class="label">{data.label}</div>
  {#if data.sublabel}<div class="sublabel">{data.sublabel}</div>{/if}
</div>

<Handle id="t-in" type="target" position={Position.Top} />
<Handle id="t-out" type="source" position={Position.Top} />
<Handle id="l-in" type="target" position={Position.Left} />
<Handle id="l-out" type="source" position={Position.Left} />
<Handle id="r-in" type="target" position={Position.Right} />
<Handle id="r-out" type="source" position={Position.Right} />
<Handle id="b-in" type="target" position={Position.Bottom} />
<Handle id="b-out" type="source" position={Position.Bottom} />

<style>
  .box {
    min-width: 110px;
    max-width: 190px;
    padding: 6px 8px;
    background: var(--bg);
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    color: var(--fg);
    font-family: var(--font-mono);
    /* Canvas graphic, not page typography: these !important metrics beat
       the app-wide uniform-cell reset inside the zooming flow canvas. */
    font-size: 12px !important;
    line-height: 1.35 !important;
    text-align: center;
  }

  .box[data-kind='proc'] {
    background: var(--code);
    border-color: var(--fg-faint);
  }

  .box[data-kind='artifact'] {
    background: var(--bg);
    border-style: dashed;
  }

  .box[data-kind='agent'] {
    background: var(--bg);
    border-color: var(--fg-muted);
    border-style: double;
    border-width: 3px;
  }

  .box[data-kind='gate'] {
    background: var(--code);
    border-color: var(--fg-muted);
    border-width: 1px;
    border-style: solid;
    transform: skewX(-8deg);
  }

  .box[data-kind='gate'] .label,
  .box[data-kind='gate'] .kicker,
  .box[data-kind='gate'] .sublabel {
    transform: skewX(8deg);
  }

  .kicker {
    font-family: var(--font-mono);
    font-size: 9.5px !important;
    line-height: 1.3 !important;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--fg-faint);
    margin-bottom: 2px;
  }

  .label {
    font-weight: 700;
    color: var(--fg);
  }

  .label :global(code) {
    font-family: var(--font-mono);
    font-size: 11px !important;
    color: var(--fg);
  }

  .sublabel {
    margin-top: 3px;
    font-family: var(--font-mono);
    font-size: 10.5px !important;
    line-height: 1.3 !important;
    color: var(--fg-muted);
  }

  /* Hide the handle dots; we still want the connection points to work. */
  :global(.svelte-flow__handle) {
    background: transparent;
    border: 0;
    width: 1px;
    height: 1px;
    min-width: 0;
    min-height: 0;
  }
</style>
