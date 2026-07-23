mod changeset;
mod children;
mod conflict;
mod engine;
mod items;
mod lines;
mod mapping;
mod trees;

pub use changeset::{ChangeSet, PcsTriple};
pub use conflict::{Conflict, Region, Result};
pub use engine::{Config, ThreeWay, ThreeWayNodes, ThreeWayParams, ThreeWayTrees};
pub use lines::based;
pub use mapping::Class;

#[cfg(test)]
mod tests;
