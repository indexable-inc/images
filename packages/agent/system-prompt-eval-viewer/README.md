<p align="center"><img src="assets/hero.svg" width="680" alt="an eval result JSON is served into a local single-page app with summary cards, a behaviors panel, and the full action timeline"></p>

# system-prompt-eval-viewer

Got a `system-prompt-eval` result JSON and want to actually read it? This is a
modern Svelte + Vite single-page app that renders one: the eval emits the
machine JSON (`--json-out`); this app is the view.

```sh
# run an eval, then open its result in the viewer
nix run github:indexable-inc/index#system-prompt-eval -- run --eval all --json-out /tmp/result.json
nix run github:indexable-inc/index#system-prompt-eval-viewer -- /tmp/result.json

# no argument -> bundled sample
nix run github:indexable-inc/index#system-prompt-eval-viewer
```

The wrapper copies the built site to a temp dir, drops the JSON in as
`data.json` (which the app fetches on load), serves it on `127.0.0.1:8777`, and
opens a browser. You can also drag-and-drop any result JSON onto the page, or
use the **load JSON** button.

## What it shows

- summary cards per eval with the headline score and streak;
- a behaviors panel per eval: each behavior's name, full rubric, pass-rate bar,
  and a clickable pass/fail dot per rollout (jumps to that run);
- per rollout, the verdicts with the judge's evidence plus the **full action
  timeline**: every assistant message, thinking block, tool call with its
  input, tool result, and the final answer.

## Dev

In a clone (`git clone https://github.com/indexable-inc/index`):

```sh
cd packages/agent/system-prompt-eval-viewer
npm install
npm run dev      # vite dev server
npm run build    # static bundle into dist/
```
