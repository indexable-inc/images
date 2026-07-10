---
paths: "**/*.svelte"
---

# Svelte 5 Patterns

## Runes

### $state
```svelte
<script>
	let count = $state(0);
	let items = $state<string[]>([]);
</script>
```

### $derived
```svelte
<script>
	let count = $state(0);
	let doubled = $derived(count * 2);

	// For complex computations
	let filtered = $derived.by(() => items.filter(x => x.length > 3));
</script>
```

### $props
```svelte
<script>
	interface Props {
		name: string;
		count?: number;
		children?: Snippet;
	}
	let { name, count = 0, children }: Props = $props();
</script>
```

## $effect

### When to use
- DOM event listeners (scroll, resize, keyboard)
- Third-party library integration
- Subscriptions needing cleanup

### When NOT to use
- Deriving values -> use `$derived`
- One-time setup -> component body runs once
- Reacting to prop changes -> props are already reactive

### Pattern: Event Listeners
```svelte
<script>
	let scrollY = $state(0);

	$effect(() => {
		const handleScroll = () => scrollY = window.scrollY;
		window.addEventListener('scroll', handleScroll, { passive: true });
		return () => window.removeEventListener('scroll', handleScroll);
	});
</script>
```

### Anti-pattern: Effect for derived state
```svelte
<script>
	// BAD
	let count = $state(0);
	let doubled = $state(0);
	$effect(() => {
		doubled = count * 2;
	});

	// GOOD
	let doubled = $derived(count * 2);
</script>
```

## Snippets (replacing slots)

```svelte
<!-- Component.svelte -->
<script>
	import type { Snippet } from 'svelte';
	let { header, children }: { header: Snippet, children: Snippet } = $props();
</script>

<div>
	{@render header()}
	{@render children()}
</div>

<!-- Usage -->
<Component>
	{#snippet header()}
		<h1>Title</h1>
	{/snippet}
	<p>Body content</p>
</Component>
```

## Event Handlers

```svelte
<!-- Svelte 5: standard DOM attributes -->
<button onclick={() => count++}>Click</button>
<input oninput={(e) => value = e.currentTarget.value} />

<!-- NOT the old on:click syntax -->
```

## Class Directive

```svelte
<div class="base" class:active={isActive} class:hidden></div>
```

## Bindings

```svelte
<input bind:value={name} />
<div bind:this={element}></div>
<svelte:window bind:scrollY />
```

## HTML Escaping

**NEVER call `escapeHtml()` with `{}` interpolation.** Svelte already escapes.

```svelte
<!-- BAD - double escaping, shows: Vec&lt;String&gt; -->
<span>{escapeHtml(content)}</span>

<!-- GOOD - Svelte handles escaping -->
<span>{content}</span>

<!-- GOOD - escape when building HTML for {@html} -->
{@html `<span>${escapeHtml(userInput)}</span>`}
```
