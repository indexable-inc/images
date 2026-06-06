<script lang="ts">
  // Profile picture. Three rendering paths, in priority order:
  //
  //   1. dataUrl  — github.com PNG already resolved through
  //                 `lib/avatarBytes` into a data: URL. Stable src,
  //                 no re-fetch on presence updates, no WebKit
  //                 onerror flicker.
  //   2. img src  — fallback used while the fetch is in flight OR
  //                 after it returned null (CORS, redirect, 404, or
  //                 the Tauri webview at tauri://localhost where the
  //                 fetch can't read github.com's bytes even though
  //                 the browser can still paint them). Rendering an
  //                 <img> doesn't require CORS, only reading pixels
  //                 from JS does, so this path renders the avatar in
  //                 every case where the URL is reachable.
  //   3. svg      — deterministic identicon derived from the display
  //                 name. Used when github is unset, or when the img
  //                 fallback itself errors (private profile, 404,
  //                 offline).

  import { untrack } from 'svelte';
  import { avatarSvg } from '$lib/avatar';
  import { loadGithubAvatar, peekGithubAvatar } from '$lib/avatarBytes';

  interface Props {
    name: string;
    seed?: string;
    size?: number;
    /** When set, render the github.com profile picture for this
     *  handle and fall back to the identicon only on fetch failure. */
    github?: string | null;
  }

  let { name, seed, size = 32, github = null }: Props = $props();

  let svg = $derived(avatarSvg(seed ?? name));
  let dataUrl = $state<string | null | undefined>(
    untrack(() => peekGithubAvatar(github, size))
  );
  // Set to true once the <img> fallback errors, so we stop trying to
  // render a broken-image icon and let the identicon take over. Reset
  // whenever the handle changes so a rename retries.
  let imgFailed = $state(false);

  // Github PNG at 2× the slot for retina. encodeURIComponent guards
  // against handles with characters that would slip through (Loro
  // peers, ?as= overrides) even though valid github handles never need
  // it.
  let fallbackUrl = $derived(
    github
      ? `https://github.com/${encodeURIComponent(github)}.png?size=${Math.ceil(size * 2)}`
      : ''
  );

  $effect(() => {
    if (!github) {
      dataUrl = null;
      imgFailed = false;
      return;
    }
    imgFailed = false;
    const peeked = peekGithubAvatar(github, size);
    if (peeked !== undefined) {
      dataUrl = peeked;
      return;
    }
    dataUrl = undefined;
    let cancelled = false;
    loadGithubAvatar(github, size).then((u) => {
      if (!cancelled) dataUrl = u;
    });
    return () => {
      cancelled = true;
    };
  });
</script>

<span
  class="avatar"
  style="width: {size}px; height: {size}px;"
  role="img"
  aria-label={name}
>
  {#if dataUrl}
    <img
      src={dataUrl}
      width={size}
      height={size}
      alt=""
      decoding="async"
    />
  {:else if github && !imgFailed}
    <img
      src={fallbackUrl}
      width={size}
      height={size}
      alt=""
      decoding="async"
      referrerpolicy="no-referrer"
      onerror={() => (imgFailed = true)}
    />
  {:else}
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    {@html svg}
  {/if}
</span>

<style>
  .avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 999px;
    overflow: hidden;
    line-height: 0;
    user-select: none;
    flex-shrink: 0;
    background: var(--bg-pill);
  }
  .avatar :global(svg),
  .avatar img {
    width: 100%;
    height: 100%;
    display: block;
    object-fit: cover;
  }
</style>
