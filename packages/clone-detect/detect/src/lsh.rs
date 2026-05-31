use std::hash::{Hash, Hasher};

use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet, FxHasher};

/// Number of hash functions in the MinHash signature.
pub const NUM_HASHES: usize = 64;

/// Number of bands for LSH banding.
const NUM_BANDS: usize = 16;

/// Rows per band = NUM_HASHES / NUM_BANDS.
const ROWS_PER_BAND: usize = NUM_HASHES / NUM_BANDS;

/// Maximum bucket size before we skip a bucket (too noisy to be useful).
const MAX_BUCKET_SIZE: usize = 200;

/// Seed generation multiplier (SplitMix64 gamma constant).
const SEED_MUL: u64 = 0x517c_c1b7_2722_0a95;

/// Seed generation addend (SplitMix64 increment).
const SEED_ADD: u64 = 0x6c62_272e_07bb_0142;

/// Pre-generated seeds for MinHash hash functions.
/// Computed at compile time via const fn; indexing is safe because
/// `i` is bounded by `NUM_HASHES` which equals the array length.
#[expect(
    clippy::indexing_slicing,
    reason = "const fn with i < NUM_HASHES = array len"
)]
const HASH_SEEDS: [u64; NUM_HASHES] = {
    let mut seeds = [0u64; NUM_HASHES];
    let mut i = 0;
    while i < NUM_HASHES {
        seeds[i] = (i as u64).wrapping_mul(SEED_MUL).wrapping_add(SEED_ADD);
        i += 1;
    }
    seeds
};

/// Compute a MinHash signature from a set of features.
#[must_use]
pub fn minhash_signature(features: &[u64]) -> [u64; NUM_HASHES] {
    let mut signature = [u64::MAX; NUM_HASHES];

    for &feature in features {
        for (sig, seed) in signature.iter_mut().zip(HASH_SEEDS.iter()) {
            let h = mix(feature, *seed);
            *sig = (*sig).min(h);
        }
    }

    signature
}

/// SplitMix64 finalizer constant (first multiply).
const MIX_MUL_1: u64 = 0xff51_afd7_ed55_8ccd;

/// SplitMix64 finalizer constant (second multiply).
const MIX_MUL_2: u64 = 0xc4ce_b9fe_1a85_ec53;

/// SplitMix64 shift width.
const MIX_SHIFT: u32 = 32;

/// Mix a value with a seed using splitmix64 finalizer.
#[inline]
fn mix(value: u64, seed: u64) -> u64 {
    let mut h = value ^ seed;
    h = h.wrapping_mul(MIX_MUL_1);
    h ^= h >> MIX_SHIFT;
    h = h.wrapping_mul(MIX_MUL_2);
    h ^= h >> MIX_SHIFT;
    h
}

/// A node location: file_id + node_idx.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeLocation {
    pub file_id: usize,
    pub node_idx: usize,
}

/// A node location paired with its MinHash signature.
pub struct LshEntry {
    pub location: NodeLocation,
    pub signature: [u64; NUM_HASHES],
}

/// An ordered pair of node locations (canonical: first <= second).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodePair {
    pub first: NodeLocation,
    pub second: NodeLocation,
}

/// LSH index for efficient candidate-pair generation.
pub struct LshIndex {
    bands: Vec<FxHashMap<u64, Vec<NodeLocation>>>,
    signatures: FxHashMap<NodeLocation, [u64; NUM_HASHES]>,
}

impl LshIndex {
    /// Build an LSH index from nodes with their MinHash signatures.
    #[must_use]
    pub fn build(entries: &[LshEntry]) -> Self {
        let mut bands: Vec<FxHashMap<u64, Vec<NodeLocation>>> = Vec::with_capacity(NUM_BANDS);
        for _ in 0..NUM_BANDS {
            bands.push(FxHashMap::default());
        }

        let mut signatures = FxHashMap::with_capacity_and_hasher(entries.len(), FxBuildHasher);

        for entry in entries {
            signatures.insert(entry.location, entry.signature);
            for (band_idx, band_map) in bands.iter_mut().enumerate() {
                let band_hash = hash_band(&entry.signature, band_idx);
                band_map.entry(band_hash).or_default().push(entry.location);
            }
        }

        Self { bands, signatures }
    }

    /// Return all candidate pairs that hash to the same bucket in at least one band.
    #[must_use]
    pub fn candidate_pairs(&self) -> Vec<NodePair> {
        let mut seen: FxHashSet<NodePair> = FxHashSet::default();
        let mut pairs = Vec::new();

        for band_map in &self.bands {
            for bucket in band_map.values() {
                if bucket.len() < 2 || bucket.len() > MAX_BUCKET_SIZE {
                    continue;
                }
                for (i, &a) in bucket.iter().enumerate() {
                    for &b in bucket.iter().skip(i + 1) {
                        let key = canonical_pair(a, b);
                        if seen.insert(key) {
                            pairs.push(key);
                        }
                    }
                }
            }
        }

        pairs
    }

    /// Look up the MinHash signature for a node location.
    #[must_use]
    pub fn signature(&self, loc: &NodeLocation) -> Option<&[u64; NUM_HASHES]> {
        self.signatures.get(loc)
    }
}

/// Estimate Jaccard similarity from MinHash signatures.
/// Counts matching slots: `matches / NUM_HASHES`. Zero allocation.
#[must_use]
pub fn estimated_jaccard(sig_a: &[u64; NUM_HASHES], sig_b: &[u64; NUM_HASHES]) -> f64 {
    let matches = sig_a
        .iter()
        .zip(sig_b.iter())
        .filter(|(a, b)| a == b)
        .count();
    matches as f64 / NUM_HASHES as f64
}

fn canonical_pair(a: NodeLocation, b: NodeLocation) -> NodePair {
    if a.file_id < b.file_id || (a.file_id == b.file_id && a.node_idx <= b.node_idx) {
        NodePair {
            first: a,
            second: b,
        }
    } else {
        NodePair {
            first: b,
            second: a,
        }
    }
}

fn hash_band(signature: &[u64; NUM_HASHES], band_idx: usize) -> u64 {
    let start = band_idx * ROWS_PER_BAND;
    let mut hasher = FxHasher::default();
    for val in signature.iter().skip(start).take(ROWS_PER_BAND) {
        val.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minhash_identical_sets_produce_identical_signatures() {
        let a = vec![1, 2, 3, 4, 5];
        let sig_a = minhash_signature(&a);
        let sig_b = minhash_signature(&a);
        assert_eq!(sig_a, sig_b);
    }

    #[test]
    fn minhash_different_sets_produce_different_signatures() {
        let sig_a = minhash_signature(&[1, 2, 3]);
        let sig_b = minhash_signature(&[100, 200, 300]);
        assert_ne!(sig_a, sig_b);
    }

    #[test]
    fn finds_similar_candidates() {
        let features_a = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let features_b = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 11]; // 90% overlap
        let features_c = vec![100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];

        let sig_a = minhash_signature(&features_a);
        let sig_b = minhash_signature(&features_b);
        let sig_c = minhash_signature(&features_c);

        let loc_a = NodeLocation {
            file_id: 0,
            node_idx: 0,
        };
        let loc_b = NodeLocation {
            file_id: 1,
            node_idx: 0,
        };
        let loc_c = NodeLocation {
            file_id: 2,
            node_idx: 0,
        };

        let entries = vec![
            LshEntry {
                location: loc_a,
                signature: sig_a,
            },
            LshEntry {
                location: loc_b,
                signature: sig_b,
            },
            LshEntry {
                location: loc_c,
                signature: sig_c,
            },
        ];
        let index = LshIndex::build(&entries);
        let pairs = index.candidate_pairs();

        let has_ab = pairs.contains(&canonical_pair(loc_a, loc_b));
        assert!(has_ab, "Similar items A and B should be candidates");

        let has_ac = pairs.contains(&canonical_pair(loc_a, loc_c));
        assert!(!has_ac, "Dissimilar items A and C should not be candidates");
    }

    #[test]
    fn estimated_jaccard_identical() {
        let sig = minhash_signature(&[1, 2, 3]);
        let est = estimated_jaccard(&sig, &sig);
        assert!((est - 1.0).abs() < 0.001);
    }

    #[test]
    fn estimated_jaccard_different() {
        let sig_a = minhash_signature(&[1, 2, 3]);
        let sig_b = minhash_signature(&[100, 200, 300]);
        let est = estimated_jaccard(&sig_a, &sig_b);
        assert!(est < 0.2, "Disjoint sets should have low estimated Jaccard");
    }
}
