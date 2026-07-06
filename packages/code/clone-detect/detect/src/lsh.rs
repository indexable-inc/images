use std::hash::{Hash, Hasher};

use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet, FxHasher};

/// Number of hash functions in the `MinHash` signature.
///
/// Highly composite (2^6) so [`banding_for_threshold`] has many `b*r = 64`
/// factorizations to tune the LSH S-curve against a configured threshold.
pub const NUM_HASHES: usize = 64;

/// Maximum bucket size before we skip a bucket (too noisy to be useful).
const MAX_BUCKET_SIZE: usize = 200;

/// LSH banding parameters: `num_bands` bands of `rows_per_band` rows each,
/// with `num_bands * rows_per_band == NUM_HASHES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Banding {
    pub num_bands: usize,
    pub rows_per_band: usize,
}

/// Derive `(bands, rows)` from the configured similarity threshold.
///
/// With MinHash-LSH banding, a pair of similarity `s` becomes a candidate with
/// probability `1 - (1 - s^r)^b`; the S-curve inflection ("LSH threshold") sits
/// near `t ≈ (1/b)^(1/r)`. Pairs above `t` are almost always surfaced, pairs
/// below it almost never. So `t` MUST sit at or just below the configured
/// similarity threshold: if `t` is higher, every pair between the two is
/// silently dropped before verification (pure recall loss); if `t` is far
/// lower, we waste work verifying false candidates.
///
/// The previous hard-coded 16×4 banding pinned `t = (1/16)^(1/4) ≈ 0.5`, so any
/// configured threshold below ~0.5 could never surface a candidate. We instead
/// enumerate every `b*r = NUM_HASHES` factorization and pick the one whose `t`
/// is the largest value not exceeding the target (falling back to the smallest
/// `t` if the target is below all of them, i.e. very low thresholds), keeping
/// the signature size constant.
#[must_use]
pub fn banding_for_threshold(threshold: f64) -> Banding {
    // Clamp to a sane open interval; t is only defined for 0 < s < 1, and the
    // caller's threshold is a similarity ratio.
    let target = threshold.clamp(0.01, 0.99);

    let mut best: Option<(Banding, f64)> = None;
    let mut fallback: Option<(Banding, f64)> = None;

    let mut rows_per_band = 1usize;
    while rows_per_band <= NUM_HASHES {
        if NUM_HASHES.is_multiple_of(rows_per_band) {
            let num_bands = NUM_HASHES / rows_per_band;
            let banding = Banding {
                num_bands,
                rows_per_band,
            };
            // t = (1/b)^(1/r)
            #[expect(
                clippy::cast_precision_loss,
                reason = "num_bands <= NUM_HASHES = 64, exact in f64"
            )]
            let t = (1.0 / num_bands as f64).powf(1.0 / rows_per_band as f64);

            // Track the smallest achievable t as a fallback for targets below
            // every t (keeps recall high rather than dropping everything).
            if fallback.is_none_or(|(_, ft)| t < ft) {
                fallback = Some((banding, t));
            }

            // Prefer the largest t that stays at or below the target: closest
            // fit from below minimizes false candidates without losing recall.
            if t <= target && best.is_none_or(|(_, bt)| t > bt) {
                best = Some((banding, t));
            }
        }
        rows_per_band += 1;
    }

    best.or(fallback)
        .map_or(
            // NUM_HASHES >= 1 guarantees at least one factorization, so this is
            // unreachable; keep a defined value rather than panic.
            Banding {
                num_bands: NUM_HASHES,
                rows_per_band: 1,
            },
            |(banding, _)| banding,
        )
}

/// Seed generation multiplier (`SplitMix64` gamma constant).
const SEED_MUL: u64 = 0x517c_c1b7_2722_0a95;

/// Seed generation addend (`SplitMix64` increment).
const SEED_ADD: u64 = 0x6c62_272e_07bb_0142;

/// Pre-generated seeds for `MinHash` hash functions.
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

/// Compute a `MinHash` signature from a set of features.
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

/// `SplitMix64` finalizer constant (first multiply).
const MIX_MUL_1: u64 = 0xff51_afd7_ed55_8ccd;

/// `SplitMix64` finalizer constant (second multiply).
const MIX_MUL_2: u64 = 0xc4ce_b9fe_1a85_ec53;

/// `SplitMix64` shift width.
const MIX_SHIFT: u32 = 32;

/// Mix a value with a seed using splitmix64 finalizer.
#[inline]
const fn mix(value: u64, seed: u64) -> u64 {
    let mut h = value ^ seed;
    h = h.wrapping_mul(MIX_MUL_1);
    h ^= h >> MIX_SHIFT;
    h = h.wrapping_mul(MIX_MUL_2);
    h ^= h >> MIX_SHIFT;
    h
}

/// A node location: `file_id` + `node_idx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeLocation {
    pub file_id: usize,
    pub node_idx: usize,
}

/// A node location paired with its `MinHash` signature.
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
    banding: Banding,
}

impl LshIndex {
    /// Build an LSH index from nodes with their `MinHash` signatures, banding
    /// the signature per `banding` (derived from the configured threshold via
    /// [`banding_for_threshold`]).
    #[must_use]
    pub fn build(entries: &[LshEntry], banding: Banding) -> Self {
        let mut bands: Vec<FxHashMap<u64, Vec<NodeLocation>>> =
            Vec::with_capacity(banding.num_bands);
        for _ in 0..banding.num_bands {
            bands.push(FxHashMap::default());
        }

        let mut signatures = FxHashMap::with_capacity_and_hasher(entries.len(), FxBuildHasher);

        for entry in entries {
            signatures.insert(entry.location, entry.signature);
            for (band_idx, band_map) in bands.iter_mut().enumerate() {
                let band_hash = hash_band(&entry.signature, band_idx, banding.rows_per_band);
                band_map.entry(band_hash).or_default().push(entry.location);
            }
        }

        Self {
            bands,
            signatures,
            banding,
        }
    }

    /// The banding this index was built with.
    #[must_use]
    pub const fn banding(&self) -> Banding {
        self.banding
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

    /// Look up the `MinHash` signature for a node location.
    #[must_use]
    pub fn signature(&self, loc: &NodeLocation) -> Option<&[u64; NUM_HASHES]> {
        self.signatures.get(loc)
    }
}

/// Estimate Jaccard similarity from `MinHash` signatures.
/// Counts matching slots: `matches / NUM_HASHES`. Zero allocation.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "match count and NUM_HASHES are tiny, well within f64 precision"
)]
pub fn estimated_jaccard(sig_a: &[u64; NUM_HASHES], sig_b: &[u64; NUM_HASHES]) -> f64 {
    let matches = sig_a
        .iter()
        .zip(sig_b.iter())
        .filter(|(a, b)| a == b)
        .count();
    matches as f64 / NUM_HASHES as f64
}

/// Estimate the overlap coefficient (containment) from `MinHash` signatures
/// plus the exact *distinct* (set) feature counts, in O(`NUM_HASHES`).
///
/// `MinHash` only estimates Jaccard `J = I/U`, but with the exact sizes and
/// `U = |A| + |B| - I` the intersection follows: `I = J(|A| + |B|)/(1 + J)`,
/// hence `overlap = I / min(|A|, |B|)`. Without this, the overlap metric has no
/// cheap candidate prune (a low-Jaccard pair can still be high-overlap), and
/// every LSH candidate pays the exact O(n+m) merge — measured as a >500x
/// slowdown over this whole repo versus the pruned Jaccard path.
///
/// The signature estimates *set* Jaccard, so `len_a`/`len_b` MUST be distinct
/// counts to keep the estimate internally consistent; feeding multiset lengths
/// under-estimates duplicate-heavy fragments and wrongly prunes them. The
/// confirmed metric is still multiset overlap, whose residual divergence from
/// the set estimate is absorbed by the caller's slack margin (estimates only
/// prune, never confirm).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "feature counts are far below f64 mantissa precision"
)]
pub fn estimated_overlap(
    sig_a: &[u64; NUM_HASHES],
    sig_b: &[u64; NUM_HASHES],
    len_a: usize,
    len_b: usize,
) -> f64 {
    let min_len = len_a.min(len_b);
    if min_len == 0 {
        return 0.0;
    }
    let j = estimated_jaccard(sig_a, sig_b);
    let intersection = j * (len_a + len_b) as f64 / (1.0 + j);
    (intersection / min_len as f64).min(1.0)
}

const fn canonical_pair(a: NodeLocation, b: NodeLocation) -> NodePair {
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

fn hash_band(signature: &[u64; NUM_HASHES], band_idx: usize, rows_per_band: usize) -> u64 {
    let start = band_idx * rows_per_band;
    let mut hasher = FxHasher::default();
    for val in signature.iter().skip(start).take(rows_per_band) {
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
        let index = LshIndex::build(&entries, banding_for_threshold(0.7));
        let pairs = index.candidate_pairs();

        let similar_pair = pairs.contains(&canonical_pair(loc_a, loc_b));
        assert!(similar_pair, "Similar items A and B should be candidates");

        let dissimilar_pair = pairs.contains(&canonical_pair(loc_a, loc_c));
        assert!(
            !dissimilar_pair,
            "Dissimilar items A and C should not be candidates"
        );
    }

    #[test]
    fn banding_is_always_a_valid_factorization() {
        for pct in 1..=99 {
            let b = banding_for_threshold(f64::from(pct) / 100.0);
            assert_eq!(
                b.num_bands * b.rows_per_band,
                NUM_HASHES,
                "banding for {pct}% must factor NUM_HASHES"
            );
        }
    }

    /// A higher configured threshold demands a higher LSH S-curve threshold,
    /// which means more rows per band (fewer bands). The banding must respond
    /// monotonically so it neither over- nor under-generates candidates.
    #[test]
    fn banding_rows_grow_with_threshold() {
        let low = banding_for_threshold(0.3);
        let high = banding_for_threshold(0.9);
        assert!(
            high.rows_per_band >= low.rows_per_band,
            "higher threshold should use >= rows per band: {low:?} vs {high:?}"
        );
    }

    /// The threshold-derived banding for a low target must place its S-curve
    /// inflection below that target, or pairs between the inflection and the
    /// target are silently dropped. The old fixed 16x4 banding pinned the
    /// inflection at 0.5, so threshold 0.4 was unreachable.
    #[test]
    fn low_threshold_lowers_lsh_inflection() {
        let b = banding_for_threshold(0.4);
        #[expect(clippy::cast_precision_loss, reason = "counts exact in f64")]
        let t = (1.0 / b.num_bands as f64).powf(1.0 / b.rows_per_band as f64);
        assert!(
            t <= 0.4,
            "derived inflection {t:.3} must sit at/below target 0.4; banding {b:?}"
        );
        // The old hard-coded banding could not: its inflection is exactly 0.5.
        let old_t = (1.0f64 / 16.0).powf(1.0 / 4.0);
        assert!(old_t > 0.4, "old 16x4 inflection {old_t:.3} exceeds 0.4");
    }

    /// Regression for the silent recall floor. Build many independent
    /// ~0.45-similar pairs and count how many each banding surfaces. `MinHash`
    /// collision is probabilistic per pair, so we assert on the aggregate: the
    /// threshold-0.4 derived banding must surface nearly all of them, while the
    /// old 16x4 banding (inflection 0.5, above 0.45) surfaces materially fewer.
    /// Set Jaccard here is 11/(11+7+7)=0.44.
    #[test]
    fn low_threshold_surfaces_borderline_pairs() {
        const PAIRS: usize = 40;
        let shared_size = 11u64;
        let unique_size = 7u64;

        let mut entries = Vec::new();
        let mut want = Vec::new();
        for p in 0..PAIRS {
            let base = (p as u64) * 1000;
            let shared: Vec<u64> = (0..shared_size).map(|k| base + k).collect();
            let mut fa = shared.clone();
            fa.extend((0..unique_size).map(|k| base + 100 + k));
            let mut fb = shared;
            fb.extend((0..unique_size).map(|k| base + 200 + k));

            let la = NodeLocation {
                file_id: p * 2,
                node_idx: 0,
            };
            let lb = NodeLocation {
                file_id: p * 2 + 1,
                node_idx: 0,
            };
            entries.push(LshEntry {
                location: la,
                signature: minhash_signature(&fa),
            });
            entries.push(LshEntry {
                location: lb,
                signature: minhash_signature(&fb),
            });
            want.push(canonical_pair(la, lb));
        }

        let count = |banding: Banding| {
            let pairs = LshIndex::build(&entries, banding).candidate_pairs();
            want.iter().filter(|w| pairs.contains(w)).count()
        };

        let derived = count(banding_for_threshold(0.4));
        let old = count(Banding {
            num_bands: 16,
            rows_per_band: 4,
        });

        assert!(
            derived >= PAIRS * 9 / 10,
            "threshold-0.4 banding must surface >=90% of ~0.45-similar pairs, got {derived}/{PAIRS}"
        );
        assert!(
            derived > old,
            "derived banding must surface strictly more than old 16x4: {derived} vs {old}"
        );
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

    /// Containment: A (10 features) fully inside B (30 features). True Jaccard
    /// is 10/30 ~ 0.33 but true overlap is 1.0; the estimate must recover the
    /// high overlap from the low Jaccard signal plus the sizes.
    #[test]
    fn estimated_overlap_recovers_containment() {
        let a: Vec<u64> = (1..=10).collect();
        let b: Vec<u64> = (1..=30).collect();
        let sig_a = minhash_signature(&a);
        let sig_b = minhash_signature(&b);

        let jac = estimated_jaccard(&sig_a, &sig_b);
        let ovl = estimated_overlap(&sig_a, &sig_b, a.len(), b.len());
        assert!(jac < 0.6, "containment pair has low Jaccard, got {jac}");
        assert!(
            ovl > 0.75,
            "estimated overlap must recover containment, got {ovl}"
        );
    }

    #[test]
    fn estimated_overlap_disjoint_stays_low() {
        let a: Vec<u64> = (1..=20).collect();
        let b: Vec<u64> = (1000..=1020).collect();
        let est = estimated_overlap(
            &minhash_signature(&a),
            &minhash_signature(&b),
            a.len(),
            b.len(),
        );
        assert!(est < 0.3, "disjoint sets must estimate low overlap: {est}");
    }

    /// Duplicate-heavy regression (review finding on #1936): the estimate must
    /// use *distinct* counts, because the signature only sees sets. Small = 30
    /// tokens each repeated 10x (multiset 300, set 30); large = the same plus
    /// 15 fresh tokens x2 (multiset 330, set 45). Exact multiset overlap is
    /// 1.0. Feeding multiset lengths deflates the estimate to ~0.84 (a 0.95
    /// threshold would wrongly prune); distinct counts keep it ~1.0.
    #[test]
    fn estimated_overlap_duplicate_heavy_containment() {
        let mut small = Vec::new();
        for token in 1..=30u64 {
            small.extend(std::iter::repeat_n(token, 10));
        }
        let mut large = small.clone();
        for token in 31..=45u64 {
            large.extend(std::iter::repeat_n(token, 2));
        }
        small.sort_unstable();
        large.sort_unstable();

        let sig_small = minhash_signature(&small);
        let sig_large = minhash_signature(&large);

        let set_based = estimated_overlap(&sig_small, &sig_large, 30, 45);
        let multiset_based =
            estimated_overlap(&sig_small, &sig_large, small.len(), large.len());
        assert!(
            set_based > 0.9,
            "set-consistent estimate must keep duplicate-heavy containment, got {set_based}"
        );
        assert!(
            set_based > multiset_based,
            "multiset lengths deflate the estimate: set {set_based} vs multiset {multiset_based}"
        );
    }

    #[test]
    fn estimated_overlap_empty_is_zero() {
        let sig = minhash_signature(&[1, 2, 3]);
        assert!(estimated_overlap(&sig, &sig, 0, 3).abs() < 0.001);
    }
}
