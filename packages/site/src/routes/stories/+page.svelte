<script lang="ts">
  import { resolve } from '$app/paths';
  import { stories } from '$lib/stories';
</script>

<svelte:head>
  <title>Stories · index</title>
  <meta name="description" content="Five short case examples of what this repo is for." />
</svelte:head>

<h1>Stories</h1>
<p class="subtitle">Five short case examples of what this repo is for.</p>

<ol class="stories">
  {#each stories as story (story.id)}
    <li>
      <a href={resolve('/stories/[id]', { id: story.id })}>{story.title}</a>
      <p class="hook">{story.hook}</p>
    </li>
  {/each}
</ol>

<style>
  .stories {
    list-style: none;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--cell-h);
  }

  .stories a {
    font-weight: 700;
  }

  .hook {
    margin: 0;
    color: var(--fg-muted);
    max-width: 65ch;
  }
</style>
