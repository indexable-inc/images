<script lang="ts">
  let { ok, href, title }: { ok: boolean; href: string; title: string } = $props();
</script>

<a class="dot {ok ? 'y' : 'n'}" {href} {title}>{ok ? '✔' : '✘'}</a>

<style>
  .dot { width: 20px; height: 20px; border-radius: 4px; display: grid; place-items: center; font-size: 11px;
    text-decoration: none; }
  .y { background: var(--good-bg); color: var(--good-fg); }
  .n { background: var(--bad-bg); color: var(--bad-fg); }
</style>
