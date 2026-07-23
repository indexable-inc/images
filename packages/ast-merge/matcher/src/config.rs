/// Maximum subtree size for tree edit distance computation.
/// Larger subtrees skip TED and rely on faster heuristics.
const DEFAULT_MAX_TED_SIZE: usize = 100;

/// Dice coefficient threshold for bottom-up matching.
const DEFAULT_DICE_THRESHOLD: f64 = 0.5;

#[derive(Debug, Clone)]
pub struct Config {
    pub min_height: usize,
    pub dice_threshold: f64,
    pub max_ted_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_height: 2,
            dice_threshold: DEFAULT_DICE_THRESHOLD,
            max_ted_size: DEFAULT_MAX_TED_SIZE,
        }
    }
}
