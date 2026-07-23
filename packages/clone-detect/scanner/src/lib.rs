mod index;
mod scan;

pub use index::{CandidateEntry, Entry, Hash, Location};
pub use scan::{Config, Error, File, Output, Scanner};

#[cfg(test)]
mod tests {
    mod directory;
    mod file;
    mod helpers;
    mod indexing;
    mod integration;
}
