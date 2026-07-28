//! Shared animation primitives for the overlays.
//!
//! The overlays are event-driven winit loops, so time-based motion drives its own
//! frames (advance toward a target by `dt / duration`, apply an easing curve,
//! redraw, sleep when at rest). This module owns the reusable pieces every
//! overlay shares: the easing curves, a per-element hover stepper
//! ([`HoverAnim`]), a continuous breathing oscillator ([`breathe`]), and a quad
//! scaler ([`scale_quads_about`]). The durations, amplitudes, and the rationale
//! live in the repo `animation` skill.

use std::time::Duration;

use crate::Quad;

/// Ease-out cubic: fast start, soft landing. The default feedback curve for
/// hovers and entrances.
pub fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Ease-out with a gentle overshoot: the value springs slightly past `1.0` and
/// settles back. Reserved for playful, non-critical surfaces (a button or arrow
/// pop), never task-critical motion. See the `animation` skill.
pub fn ease_out_back(t: f32) -> f32 {
    const C1: f32 = 1.2;
    const C3: f32 = C1 + 1.0;
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + C3 * u * u * u + C1 * u * u
}

/// A breathing oscillator: a sine in `-1..=1` driven by a continuous `elapsed`
/// clock, so it never snaps when a hover toggles. `period` is one full cycle;
/// fade the result in with a hover amount so a resting element stays still.
pub fn breathe(elapsed: Duration, period: Duration) -> f32 {
    use std::f32::consts::TAU;
    let p = period.as_secs_f32().max(f32::EPSILON);
    (TAU * elapsed.as_secs_f32() / p).sin()
}

/// A hover amount in `0..=1`, eased toward a target at a fixed rate. Keep one per
/// animated element: call [`HoverAnim::approach`] each frame with the frame's
/// `dt` and the grow/shrink `duration`, then read [`HoverAnim::raw`] or
/// [`HoverAnim::eased`] to render. The raw value is the linear progress; `eased`
/// maps it through [`ease_out_cubic`] for the visible curve.
#[derive(Clone, Copy, Default)]
pub struct HoverAnim {
    raw: f32,
}

impl HoverAnim {
    /// Step the linear progress toward `target` (`0.0` resting, `1.0` hovered) by
    /// `dt / duration`, clamped so it lands exactly on the target.
    pub fn approach(&mut self, target: f32, dt: Duration, duration: Duration) {
        let step = dt.as_secs_f32() / duration.as_secs_f32().max(f32::EPSILON);
        self.raw = if self.raw < target {
            (self.raw + step).min(target)
        } else {
            (self.raw - step).max(target)
        };
    }

    /// Linear progress in `0..=1`.
    pub fn raw(&self) -> f32 {
        self.raw
    }

    /// Progress mapped through [`ease_out_cubic`], the visible feedback curve.
    pub fn eased(&self) -> f32 {
        ease_out_cubic(self.raw)
    }

    /// True once fully eased back to rest, so the loop can stop redrawing it.
    pub fn is_resting(&self) -> bool {
        self.raw <= 0.0
    }

    /// True only while a transition is in flight (strictly between rest and fully
    /// hovered). The two settled states are `0.0` and `1.0`, so an element with no
    /// continuous motion lets the loop sleep once this is false, even while still
    /// hovered.
    pub fn is_animating(&self) -> bool {
        self.raw > 0.0 && self.raw < 1.0
    }
}

/// Scale every quad's rect about `(cx, cy)` by `mul`, growing a laid-out group in
/// place. A no-op at unit scale, so a resting overlay pays nothing.
pub fn scale_quads_about(quads: &mut [Quad], cx: f32, cy: f32, mul: f32) {
    if (mul - 1.0).abs() < f32::EPSILON {
        return;
    }
    for q in quads {
        q.x = cx + (q.x - cx) * mul;
        q.y = cy + (q.y - cy) * mul;
        q.w *= mul;
        q.h *= mul;
    }
}
