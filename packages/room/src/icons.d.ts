// unplugin-icons resolves `~icons/<set>/<name>` to a Svelte component at
// build time. This shim teaches TypeScript and svelte-check about the
// shape so each `import Foo from '~icons/lucide/foo'` is properly typed.

declare module '~icons/*' {
  import type { Component } from 'svelte';
  import type { SVGAttributes } from 'svelte/elements';
  const Icon: Component<SVGAttributes<SVGSVGElement>>;
  export default Icon;
}
