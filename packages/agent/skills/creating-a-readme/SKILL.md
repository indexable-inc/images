---
name: creating-a-readme
description: "House README style: a committed SVG hero with a dark twin selected via a picture element (Safari ignores prefers-color-scheme inside img-loaded SVGs), a hook-question intro with a 2-3 sentence pitch, install lines derived from the flake and crate metadata, and an optional git-log version history. Use when writing, reviewing, or generating any README in this repo, including the mirror generator's output."
---

## Creating a README

Every README opens with one committed SVG hero, then a pitch a human gets in
five seconds. Short sections, no walls of text, no metadiscussion about the
document itself. This skill is the single source of truth for README style;
generators (e.g. `packages/mirror`) conform to it rather than restating it.

### Hero SVG

One symbolic SVG per README, committed next to it (convention:
`assets/hero.svg`; the root README uses `doc/assets/`), plus a committed dark
twin (`assets/hero-dark.svg`): the same file with the dark palette as the
`:root` defaults and the dark media block removed. The twin exists because
Safari never evaluates `prefers-color-scheme` (or `light-dark()`) inside an
SVG loaded through `<img>` (WebKit bug 199134), so a lone self-adapting file
renders its light palette on GitHub's dark theme for every Safari reader.
Reference both with a `<picture>` as the first element; the page-level media
query switches correctly in every engine:

```markdown
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="one line: what the diagram shows">
  </picture>
</p>
```

Keep the theme switch inside the base SVG too, so it still adapts when
viewed directly:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 720 200" role="img"
     font-family="system-ui, -apple-system, 'Segoe UI', sans-serif">
  <style>
    svg { color: #1f2328; }        /* light-mode foreground */
    .muted  { fill: #656d76; }
    .accent { fill: #8250df; }
    .box    { stroke: currentColor; fill: none; }
    text    { fill: currentColor; }
    .edge   { stroke: currentColor; fill: none; marker-end: url(#arrow); }
    @media (prefers-color-scheme: dark) {
      svg { color: #e6edf3; }
      .muted  { fill: #8b949e; }
      .accent { fill: #a371f7; }
    }
  </style>
  <!-- shapes: use the classes above, never hard-coded fills -->
</svg>
```

Rules:

- Transparent background. Every themed property gets a value in both the
  default (light) block and the dark block, and the dark twin's `:root`
  defaults must equal the base file's dark block; verify by reading the CSS.
  Even where the embedded query works (Chrome, Firefox), it follows the
  OS/browser scheme, not GitHub's manual theme toggle.
- Symbolic and minimal: one diagram showing what the thing does (data flow,
  before/after, the shape of the transformation). A README needs one hero,
  not a gallery.
- Relationships are edges (arrows, lines), never prose inside node labels.
  Label nodes with a noun; let the arrows carry the verbs.
- Hand-write the SVG: a dozen shapes, real `<text>` elements (no paths of
  outlined text), no embedded rasters, no fixed `width`/`height` attributes
  (viewBox only, so it scales).

### Voice

- Open with a hook question the target reader has actually asked ("Ever
  needed to merge two SQLite files in git?"), then a 2-3 sentence pitch in
  plain words: what it is, what it does for you, why it beats the obvious
  alternative.
- ADHD-concise from there: short sections with concrete headings, code blocks
  over paragraphs, a number or path over an adjective. Cut anything the
  reader cannot act on.
- Follow the `writing-style` skill for the prose itself (no em dashes, lead
  with the answer).

### Install / usage: derive, do not hand-write

The install section is a function of what the package is. Check metadata
instead of guessing:

- Runnable flake package (`nix eval .#<attr>.meta.mainProgram` succeeds):

  ```sh
  nix run github:indexable-inc/index#<attr> -- --help
  ```

- Rust crate (`Cargo.toml` present): also give the cargo form. For a crate
  with a standalone mirror (its `package.nix` sets the `mirror` manifest
  attr; list them with `nix eval .#lib.mirrorPackages`), point cargo at the
  mirror repo:

  ```sh
  cargo install --git https://github.com/indexable-inc/<mirror-id>
  ```

  For unmirrored workspace crates, the git-dependency form against the
  monorepo (`<crate> = { git = "https://github.com/indexable-inc/index" }`)
  only works for a bin via `cargo install`; libraries stay Nix-consumed.

- Library-only packages (polars plugins, SDKs, NixOS modules): show the one
  consumption line native to the ecosystem (a `flake.nix` input, `pip
  install` of the wheel output, `imports = [ ... ]`), still derived from what
  the flake actually exposes.

Always end the section with how to get the flake itself when the commands
assume a clone: `git clone https://github.com/indexable-inc/index`.

### Version history (optional)

If a package wants a history section, generate it from git rather than
maintaining it by hand, and regenerate on touch:

```sh
git log --reverse --format='- `%h` %s' -- packages/<path>
```

Keep only releases or behavior changes a user would notice; a README is not a
changelog. Skip the section entirely for fast-moving packages.

### Checklist

1. `assets/hero.svg` plus `assets/hero-dark.svg` committed, referenced first
   via `<picture>`, legible in both schemes.
2. Hook question plus 2-3 sentence pitch above the fold.
3. Install lines derived from flake/crate metadata, not copied from another
   README.
4. Mirrored packages: README plus `assets/` must make sense standalone
   (relative links only to files that ride into the mirror).
5. index's `nix run .#lint` passes (skill-lint checks this file's frontmatter
   too).
