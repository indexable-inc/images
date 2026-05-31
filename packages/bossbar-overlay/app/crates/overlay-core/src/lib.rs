//! `overlay-core`: the reusable float window + wgpu pixel/text engine behind the
//! Minecraft-style desktop overlays.
//!
//! It owns the mechanics every overlay shares and none of the domain:
//! - a transparent, borderless, always-on-top, click-through float window with a
//!   non-activating raise ([`window`]),
//! - one textured-quad wgpu pipeline with a texture registry, plus the vanilla
//!   bitmap font so text is just more quads ([`gpu`], [`bitmap_font`]),
//! - press/drag/click disambiguation for draggable windows ([`gesture`]),
//! - a native right-click context menu to close/dismiss an overlay ([`menu`]),
//! - a headless render-to-PNG for verification ([`snapshot`]),
//! - the shared animation primitives the overlays drive their hovers with
//!   ([`anim`]: easing curves, a hover stepper, a breathe oscillator).
//!
//! A consumer (the boss bar, the book) builds a `Vec<`[`Quad`]`>` for its domain
//! and hands it to [`Gpu::draw`] or [`snapshot::render_to_png`]. The Mojang art
//! and font are sourced by the `minecraft-assets` Nix derivation; the font sheet
//! is embedded here so every overlay renders the same text.

pub mod anim;
pub mod bitmap_font;
pub mod gesture;
pub mod gpu;
pub mod menu;
pub mod snapshot;
pub mod window;

pub use anim::HoverAnim;
pub use bitmap_font::BitmapFont;
pub use gesture::DragClick;
pub use gpu::{Gpu, Quad, TexHandle, SHADOW};

// Re-export the heavy deps so consumers name the exact versions this workspace
// pins, without each crate re-declaring them.
pub use glam;
pub use wgpu;
pub use winit;
