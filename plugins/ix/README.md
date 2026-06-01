# ix plugin

Packages the ix sandbox docs as an installable [Claude Code plugin](https://code.claude.com/docs/en/plugin-marketplaces), so anyone evaluating ix can pull the docs into a session without cloning this repo:

```
/plugin marketplace add indexable-inc/index
/plugin install ix@ix
```

The skill content is single-sourced. `skills/ix` here is a symlink to the
top-level [`skills/ix`](../../skills/ix), which is the one copy of the markdown:
the Nix skill delivery (`lib/skills.nix`) reads it directly, and a Git-installed
marketplace dereferences the symlink and copies the content into its plugin
cache. Edit the docs at `skills/ix/`, never here.

Scoping the plugin to this `plugins/ix/` subtree (rather than the repo root) is
deliberate: the plugin system only scans a plugin's own directory, so the other
skills under the top-level `skills/` stay out of `/plugin install`.
