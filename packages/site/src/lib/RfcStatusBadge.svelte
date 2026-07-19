<script lang="ts">
  const { status }: { status: string } = $props();
  // 'Last call' -> 'last-call': the class hook each status colors from.
  const slug = $derived(status.toLowerCase().replace(/\s+/g, '-'));
</script>

<span class="badge {slug}">{status}</span>

<style>
  .badge {
    display: inline-block;
    font-family: var(--font-mono);
    font-size: 0.68rem;
    line-height: 1;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    padding: 0.28em 0.6em;
    border-radius: 999px;
    color: var(--c);
    background: color-mix(in srgb, var(--c) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--c) 35%, transparent);
    white-space: nowrap;
    vertical-align: middle;
  }

  /* The ladder runs faint -> saturated as an RFC gains weight:
     a sketch barely registers; load-bearing is unmissable. */
  .sketch {
    --c: #a0a0a0;
    border-style: dashed;
  }
  .draft {
    --c: #6b6b6b;
  }
  .input-wanted {
    --c: #b45309;
  }
  .last-call {
    --c: #7c3aed;
  }
  .accepted {
    --c: #1d4ed8;
  }
  .load-bearing {
    --c: #15803d;
  }
  .rejected {
    --c: #b91c1c;
  }
  .withdrawn {
    --c: #a0a0a0;
    border-style: dashed;
  }
  .superseded {
    --c: #6b6b6b;
    border-style: dashed;
  }

  @media (prefers-color-scheme: dark) {
    .sketch {
      --c: #5d5a55;
    }
    .draft {
      --c: #9a9690;
    }
    .input-wanted {
      --c: #fbbf24;
    }
    .last-call {
      --c: #a78bfa;
    }
    .accepted {
      --c: #60a5fa;
    }
    .load-bearing {
      --c: #4ade80;
    }
    .rejected {
      --c: #f87171;
    }
    .withdrawn {
      --c: #5d5a55;
    }
    .superseded {
      --c: #9a9690;
    }
  }
</style>
