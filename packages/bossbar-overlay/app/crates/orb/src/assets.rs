//! Embedded Mojang art for the experience-orb overlay. The orb sprite sheet is
//! extracted from the official Minecraft client jar by the `minecraft-assets` Nix
//! derivation and dropped into `assets/entity/` before the build (gitignored; see
//! the workspace `.gitignore`). `include_bytes!` bakes it into the binary, so
//! there is no runtime asset path to resolve.

/// The experience-orb sheet: a 64x64 image holding the 16x16 orb icons in a 4x4
/// grid, larger/brighter for more XP. The art is grey; the renderer tints it a
/// pulsing green-yellow, the shimmer the vanilla orb has. See [`crate::scene`].
pub const EXPERIENCE_ORB: &[u8] = include_bytes!("../assets/entity/experience_orb.png");
