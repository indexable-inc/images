# tmux

`packages/tmux` repackages [tmux](https://github.com/tmux/tmux) with a modern
default config baked in (truecolor, undercurl, mouse, vi copy mode, sane
history and escape-time). The tmux binary is unchanged; the delta is a config
file forced onto every launch with the user's own config still layered on top.

## What this repo changes

The wrapper is a `symlinkJoin` over upstream `tmux` plus its man output, with a
`wrapProgram` that points tmux at the repo config
(`packages/tmux/default.nix:12-23`):

```
wrapProgram $out/bin/tmux --add-flags "-f ${./tmux.conf}"
```

- `symlinkJoin` (not a bare wrapper) folds tmux's separate `man` output into the
  single `out`, because `symlinkJoin` only merges each input's default output;
  `tmux.man` is listed explicitly in `paths` so the man pages survive the wrap
  (`packages/tmux/default.nix:14-19`).
- `meta.outputsToInstall` is forced to `[ "out" ]`: this derivation has only
  `out`, and inheriting base tmux's `[ "out" "man" ]` would make buildEnv (e.g.
  `home.packages`) try to read a nonexistent `man` output and fail
  (`packages/tmux/default.nix:27-30`).
- `meta.description` is suffixed "with modern truecolor defaults baked in" and
  `mainProgram = "tmux"` (`packages/tmux/default.nix:24-26`).

### The baked config (`tmux.conf`)

These are defaults: the config sources the user's own
`~/.config/tmux/tmux.conf` and `~/.tmux.conf` last, so personal settings win
(`packages/tmux/tmux.conf:33-35`). Key settings:

- Truecolor: `default-terminal "tmux-256color"`, `terminal-features ",*:RGB"`
  (24-bit passthrough) and `,*:usstyle` (curly/colored underlines)
  (`packages/tmux/tmux.conf:9-11`). Without an RGB terminfo tmux quantizes
  24-bit color to the 256-color palette (the washed-out look inside tmux).
- `CLAUDE_CODE_TMUX_TRUECOLOR 1` set in the tmux environment
  (`packages/tmux/tmux.conf:16`): Claude Code clamps its own rendering to
  256-color whenever `$TMUX` is set (anthropics/claude-code#36785); this flips
  its documented escape hatch on for every pane since the config already passes
  real truecolor.
- Responsiveness: `escape-time 10`, `focus-events on`, `repeat-time 600`
  (`packages/tmux/tmux.conf:19-21`).
- Comfort: `mouse on`, `history-limit 50000`, `set-clipboard on` (OSC 52),
  `base-index 1` / `pane-base-index 1`, `renumber-windows on`, `mode-keys vi`,
  `allow-passthrough on` (`packages/tmux/tmux.conf:24-31`).

## Build and wiring

- Flake output: `nix run .#tmux` / `nix build .#tmux`. `package.nix` sets
  `packageSet = true`, `flake = true` (`packages/tmux/package.nix:1-5`); no
  overlay, so `pkgs.tmux` stays stock.
- The binary, version, and platforms all come from upstream `tmux` (the
  derivation name is `tmux-${tmux.version}`); there is no separate source pin
  or updater. Editing `tmux.conf` rebuilds the wrapper.
