//! Embedded Mojang art for the overlay. The sprites are extracted from the
//! official Minecraft client jar by the `minecraft-assets` Nix derivation and
//! dropped into `assets/` before the build (gitignored; see the workspace
//! `.gitignore`). `include_bytes!` bakes them into the binary, so there is no
//! runtime asset path to resolve.

/// The experience-orb sheet: a 64x64 image holding the 16x16 orb icons in a 4x4
/// grid, larger/brighter for more XP. The art is grey; the renderer tints it a
/// pulsing green-yellow, the shimmer the vanilla orb has. Used for the `orb`
/// (success) pop kind. See [`crate::scene`].
pub const EXPERIENCE_ORB: &[u8] = include_bytes!("../assets/entity/experience_orb.png");

/// The angry-villager particle: an 8x8 grey "displeased / can't trade" puff (the
/// `angry_villager` particle's `minecraft:angry` texture). Used for the `villager`
/// (failure) pop kind, drawn at the same on-screen footprint as the orb. See
/// [`crate::scene`].
pub const ANGRY_VILLAGER: &[u8] = include_bytes!("../assets/particle/angry.png");
