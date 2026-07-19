---
name: visual-explainers
description: "Build interactive visual explanations: single-file HTML explorables, system diagrams, and data charts where the reader learns by doing. Use whenever asked to visualize something, explain a system or mechanism visually, build an interactive or explorable explainer, diagram a pipeline or protocol, or chart data. Encodes the house philosophy: the visualization is the explanation, no decorative animation, progressive disclosure, one accent color, a single self-contained HTML file in vanilla JS."
---

## Visual explainers

Make rich, simple, beautiful visualizations. The goal is that the reader walks
away with a deep understanding of the system, gained by interacting with it,
explained in plain words. Everything below serves that.

## The visualization is the explanation

The reader learns by doing (edit, click, toggle, drag), not by watching. If the
page would teach the same thing as a static screenshot of itself, it is a
diagram with extra steps; add the interaction that makes the reader's action
the lesson.

Every moving pixel must satisfy one of two tests: the reader caused it, or it
encodes causality in the system being explained (pipeline stages lighting in
the order they actually fire after the reader hits save). Delete everything
else: entrance transitions, pulse loops, parallax, hover shimmer, animated
gradients. No animation for animation's sake.

## Progressive disclosure

Overview first. One idea per view. Details on demand: clicking a node, stage,
or mark reveals its explanation in place, next to the thing it explains. Never
dump all annotations at once; a page where every label is visible up front has
already spent the reader's attention before they act.

Include a counterexample or contrast when it deepens understanding: show the
world without the mechanism. A "full reload" button next to an HMR pipeline
demonstrates exactly the state loss HMR prevents, and teaches more than a
paragraph saying so.

## Deep but simple

Plain words, short sentences. Use real payloads, real file contents, and real
values from the actual system under discussion, never lorem ipsum or `foo`.
The test of success is prediction, not recall: after using the page, the
reader should be able to say what the system would do in a case the page never
showed.

## Visual system

- Dark-friendly. One accent color, plus at most one or two semantic colors
  (error red, success green) used only for their meaning.
- Generous whitespace. Monospace for code and data tokens.
- No chart junk, no gradients as decoration, no drop shadows doing nothing.

## Form

Default to a single self-contained HTML file the user can open locally: inline
CSS and JS, no build step, no CDN fetches. Vanilla JS unless the task genuinely
demands more. State lives in a few plain variables; a render function reflects
it. This keeps the artifact portable, diffable, and editable by the reader.

## Data charts

When the task is an actual data chart rather than a system explainer:

- Honest axes: bar charts start at zero; never truncate an axis to manufacture
  drama; label units.
- Direct labeling over legends: put the series name at the end of its line.
- Accessible contrast for every mark and label, in dark and light.
- Interaction still earns its place: a tooltip that shows the exact value, a
  toggle that isolates one series. Nothing autoplays.

## Worked example: the target shape

"How Svelte HMR works": the reader edits a fake `Button.svelte` in a textarea,
clicks the rendered counter a few times to build up state, then hits save. The
pipeline stages (watcher, compile, patch, re-render) light in causal order,
each clickable for an in-place explanation of what it just did, and the counter
keeps its value. A "full reload" button beside it wipes the counter,
demonstrating the state loss HMR exists to prevent. Every element of that page
follows a rule above: interaction builds the state the lesson needs, motion
encodes causal order, disclosure is per-stage on click, and the contrast
button is the counterexample.

## Before shipping

- Remove any motion the reader did not cause and the system does not explain.
- Confirm the page opens from `file://` with the network disabled.
- Read every annotation: is anything visible before the reader needs it?
- Replace any placeholder data that survived with values from the real system.
