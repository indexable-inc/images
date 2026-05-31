//! Embedded Mojang GUI art for the book overlay. The book texture and page-turn
//! widgets are extracted from the official Minecraft client jar by the
//! `minecraft-assets` Nix derivation and dropped into `assets/gui/` before the
//! build (gitignored; see the workspace `.gitignore`). `include_bytes!` bakes them
//! into the binary, so there is no runtime asset path to resolve.

/// The book GUI sheet. The single-page background sits in its top-left; see
/// [`crate::scene`] for the exact source rect.
pub const BOOK: &[u8] = include_bytes!("../assets/gui/book.png");

pub const PAGE_FORWARD: &[u8] = include_bytes!("../assets/gui/page_forward.png");
pub const PAGE_BACKWARD: &[u8] = include_bytes!("../assets/gui/page_backward.png");
