---
name: animation
description: >
  Animation principles for repo-owned UIs (winit/wgpu overlays, ratatui TUIs, the web
  dashboard): pick duration and easing by intent, ease-out for entrances and feedback,
  springs for interruptible/user-driven motion, and gentle "breathing" loops via a
  continuous sine clock. Includes concrete durations, cubic-bezier and spring constants,
  reduced-motion handling, the native-Rust frame-driving pattern, and anti-patterns. Use
  whenever you add or tune motion (hover, press, enter/exit, pulse/breathe) in any
  repo-owned interface.
---

# Animation

Animate with intent: motion is another channel the interface speaks through. The
goal is not more motion, it is motion that earns its place and then disappears.
Default to **not** animating; add it where it clarifies a change, guides
attention, or makes feedback land.

## Duration is the highest-leverage knob

If an animation feels slow, it is almost never the curve — it is the duration.
Shorten time before touching easing.

- **Hover / press:** 120–180 ms
- **Small state change:** 180–260 ms
- **Larger transition:** up to 300 ms
- **> 300 ms** reads as *intentional* (a statement), not *reactive*. Fine on
  purpose, wrong for feedback.

Entrances can run slightly longer than the matching exit (e.g. 300 ms in,
200 ms out).

## Pick the curve by role

Ask first: is this motion **reacting to the user**, or is it **the system
announcing a change**?

- **Ease-out** — the default for entrances and feedback. Starts fast (feels
  responsive), softens at the end (eye catches up). `cubic-bezier(0, 0, 0.2, 1)`.
- **Ease-in** — exits. Accelerates away so a leaving element doesn't linger.
  `cubic-bezier(0.4, 0, 1, 1)`.
- **Ease-in-out** — transitions between two equally important states (view/mode
  switch). Neutral; overusing it makes everything feel sluggish and over-polite.
- **Spring** — user-driven, interruptible motion (drag, flick, press) where the
  motion must survive interruption and carry velocity. Springs have no fixed
  duration; they resolve naturally. Sweet spot to start: `stiffness 400,
  damping 15` (snappy with a hint of overshoot). Higher stiffness = snappier;
  lower damping = bouncier. Reserve overshoot/bounce (`cubic-bezier(0.34, 1.56,
  0.64, 1)`) for playful, non-critical surfaces — never task-critical UI.

Rule of thumb: drag/gesture → spring; "the system did X" → easing.

## Breathing / ambient loops (a value that should never stop)

For a pulse, a breathing idle indicator, or a hovered element that should feel
alive, you need a loop, not an event-driven tween. The shape:

- Oscillate **forward and back** (reverse on each cycle), never sawtooth-restart
  — a snap at the peak reads as a glitch.
- **Smooth easing** within each half-cycle (ease-in-out / sine), so the value
  dwells near the extremes and accelerates through the middle. Linear reads
  mechanical, like a metronome.
- **Subtle amplitude:** ±2–5% scale around the base. Cross 1.0 in both
  directions for a symmetric breathe.
- **Calm period ≈ 1.2–3 s** per full cycle (near a resting heart rate) reads
  "alive"; ~200–600 ms reads "urgent / warning."
- Implement as `sin()` of a **continuous clock** (`base * (1 + amp * sin(2π *
  t / period))`). Never reset the phase on state change, or it jumps.

Fade the breathe in with the hover/enter progress so a resting element is
perfectly still.

## What to animate

Prefer **transform (scale, translate) and opacity** — cheap and smooth.
Avoid animating layout-driving properties (width/height/x/y of large elements);
they are janky and expensive. Coordinate scale + opacity on the same clock so a
pulse reads as one object, not two effects.

## Honor reduced motion

Respect the OS "reduce motion" setting (macOS *Reduce Motion* /
`prefers-reduced-motion`): collapse animations to an instant state change rather
than removing the feedback entirely.

## Native Rust (winit/wgpu) frame-driving pattern

Event-driven render loops are idle until something happens, so a time-based
animation must drive its own frames:

- Keep a clock (`std::time::Instant`), an app `start` for the continuous breathe
  phase, and per-element eased progress (e.g. `hover_anim: f32`).
- Each frame: advance progress toward its target by `dt / duration`, apply the
  easing curve, compute scale + alpha, render.
- In `about_to_wait`, if anything is still animating, `request_redraw` it and set
  `ControlFlow::WaitUntil(now + ~16ms)` (≈60 fps); when everything is at rest,
  set `ControlFlow::Wait` so the process goes fully idle. Never busy-loop with
  `Poll`.

The boss bar overlay is the worked example:
[`packages/bossbar-overlay`](packages/bossbar-overlay) — ease-out hover grow plus
a sine breathe while hovered, frame-driven exactly as above, with the window
reserving the max-scale headroom so the bar grows in place.

## Anti-patterns

- Linear motion for anything physical — feels dead.
- Everything ease-in-out — sluggish and over-polite.
- Overshoot/bounce on task-critical surfaces — jarring; reserve for playful UI.
- Reactive feedback > 300 ms — reads as lag.
- Sawtooth-restart breathing — snaps at the peak.
- Busy-loop redraw — cap to ~60 fps and stop when the value reaches rest.

## Sources

> Easing vs springs, durations, control points:
> <https://raphaelsalaja-userinterface-wiki.mintlify.app/animation/to-spring-or-not-to-spring>
> Duration and easing in UX: <https://www.nngroup.com/articles/animation-duration/>
> Choosing easing (ease-out default, durations): <https://web.dev/articles/choosing-the-right-easing>
> Spring constants (stiffness/damping): <https://tigerabrodi.blog/how-to-implement-spring-physics-buttons-with-framer-motion>
> Breathing loop (reverse, easing, amplitude, period): <https://doveletter.dev/docs/compose-animations/pulsing-heart>
