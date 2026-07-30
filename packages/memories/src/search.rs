//! Ranked search over a loaded corpus: BM25 with the format's field boosts,
//! then the contract's score.
//!
//! There is no separate rank step. A caller that wants the raw BM25 order reads
//! `bm25` off each hit and sorts it itself, which is one result with two views
//! rather than two entry points that can disagree.

use crate::{
    discover::Corpus,
    error::{self, Result},
    model::{Genre, Memory},
    rank,
};
use chrono::{DateTime, Utc};
use file_search::{FieldBoost, MultiFieldEphemeralSearch};
use snafu::ResultExt;

/// Field boosts into BM25. A `tldr` or `handle` match is what someone means
/// when they search; a body match is corroboration.
///
/// These are real per-field boosts on a multi-field index rather than the same
/// text repeated N times in one field. Repetition needs no new code, but it
/// inflates the document length and BM25's length normalization then discounts
/// every other term in that document, so a memory with a long `tldr` would be
/// quietly penalized on its body terms.
const FIELDS: [FieldBoost; 4] = [
    FieldBoost {
        name: "tldr",
        boost: 3.0,
    },
    FieldBoost {
        name: "handle",
        boost: 3.0,
    },
    FieldBoost {
        name: "topic",
        boost: 2.0,
    },
    FieldBoost {
        name: "body",
        boost: 1.0,
    },
];

/// What to search for and which memories are eligible.
#[derive(Debug)]
pub struct Query<'a> {
    pub text: &'a str,
    pub limit: usize,
    /// A hit must carry at least one of these topics. Empty means no filter.
    pub topics: &'a [String],
    /// A hit must be one of these genres. Empty means no filter.
    pub genres: &'a [Genre],
    /// Include refuted memories, which are otherwise excluded.
    pub include_refuted: bool,
}

/// One hit: which memory, its raw BM25, and its ranked score.
#[derive(Clone, Copy, Debug)]
pub struct Ranked {
    /// Index into [`Corpus::memories`].
    pub memory: usize,
    pub bm25: f64,
    pub score: f64,
}

/// Search the corpus, returning hits ordered by score, highest first.
///
/// # Errors
///
/// Returns [`crate::Error::Search`] when the in-memory index cannot be built or
/// the query does not parse.
pub fn search(corpus: &Corpus, query: &Query<'_>, now: DateTime<Utc>) -> Result<Vec<Ranked>> {
    let candidates: Vec<usize> = corpus
        .memories
        .iter()
        .enumerate()
        .filter(|(_, memory)| eligible(memory, query))
        .map(|(index, _)| index)
        .collect();

    if candidates.is_empty() || query.limit == 0 {
        return Ok(Vec::new());
    }

    let documents = candidates
        .iter()
        .map(|&index| document(&corpus.memories[index]));
    let index = MultiFieldEphemeralSearch::from_documents(&FIELDS, documents)
        .context(error::SearchSnafu)?;

    // Ask for every candidate rather than `limit`: the score reorders BM25, so
    // truncating on BM25 first could drop the hit that ends up ranked first.
    let hits = index
        .search(query.text, candidates.len())
        .context(error::SearchSnafu)?;

    let mut ranked: Vec<Ranked> = hits
        .into_iter()
        .filter_map(|hit| {
            let &memory = candidates.get(hit.id)?;
            let bm25 = f64::from(hit.score);
            let score = rank::score(&corpus.memories[memory], bm25, now);
            // Below the floor the hit is worse than no answer: a caller that
            // gets an empty result can say "nothing is written down about this",
            // which is true and useful, where the least-bad match is neither.
            (score >= rank::MIN_SCORE).then_some(Ranked {
                memory,
                bm25,
                score,
            })
        })
        .collect();

    // Ties break on slug so the same corpus and query always print the same
    // order, whatever the directory listing happened to yield.
    ranked.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| {
            corpus.memories[left.memory]
                .slug
                .cmp(&corpus.memories[right.memory].slug)
        })
    });
    ranked.truncate(query.limit);
    Ok(ranked)
}

fn eligible(memory: &Memory, query: &Query<'_>) -> bool {
    if !query.include_refuted && memory.is_refuted() {
        return false;
    }
    if !query.topics.is_empty()
        && !memory
            .topic
            .iter()
            .any(|topic| query.topics.iter().any(|wanted| wanted == topic))
    {
        return false;
    }
    if !query.genres.is_empty() && !query.genres.contains(&memory.genre) {
        return false;
    }
    true
}

/// One document per memory, field values positional against [`FIELDS`].
fn document(memory: &Memory) -> Vec<String> {
    vec![
        memory.tldr.clone(),
        memory.handle.join(" "),
        memory.topic.join(" "),
        memory.body.clone(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{Query, search};
    use crate::{
        discover::Corpus,
        fixture::{Repo, fixed_now},
        model::Genre,
    };

    fn query(text: &str) -> Query<'_> {
        Query {
            text,
            limit: 10,
            topics: &[],
            genres: &[],
            include_refuted: false,
        }
    }

    fn slugs(corpus: &Corpus, query: &Query<'_>) -> Vec<String> {
        search(corpus, query, fixed_now())
            .expect("searching a fixture corpus")
            .into_iter()
            .map(|ranked| corpus.memories[ranked.memory].slug.clone())
            .collect()
    }

    fn corpus() -> Repo {
        let repo = Repo::new();
        repo.memory(
            "rebuild-cascade",
            "topic: [nix]\nhandle: [nix-dag]\nvalidated_today",
            "An env var holding a store path makes every dependent derivation rebuild.\n",
        );
        repo.memory(
            "kitchen-sink",
            "topic: [audio]\nvalidated_today",
            "Something else entirely about sound.\n",
        );
        repo
    }

    #[test]
    fn a_matching_memory_is_returned_and_an_unrelated_one_is_not() {
        let repo = corpus();
        assert_eq!(slugs(&repo.load(), &query("rebuild")), ["rebuild-cascade"]);
    }

    #[test]
    fn topic_and_genre_filters_narrow_the_candidate_set() {
        let repo = corpus();
        let corpus = repo.load();
        let topics = vec!["audio".to_owned()];
        let mut filtered = query("rebuild");
        filtered.topics = &topics;
        assert!(
            slugs(&corpus, &filtered).is_empty(),
            "the matching memory is not in the audio topic"
        );

        let genres = vec![Genre::Living];
        let mut by_genre = query("rebuild");
        by_genre.genres = &genres;
        assert!(
            slugs(&corpus, &by_genre).is_empty(),
            "the matching memory is a `memory`, not a `living` page"
        );
    }

    #[test]
    fn a_refuted_memory_is_excluded_until_all_is_asked_for() {
        let repo = Repo::new();
        repo.memory(
            "was-the-lockfile",
            "validated:\n  - at: 2026-07-21T00:00:00Z\n    by: t\n    how: c\n    ok: false\n",
            "An env var holding a store path makes every dependent derivation rebuild.\n",
        );
        let corpus = repo.load();
        assert!(slugs(&corpus, &query("rebuild")).is_empty());

        let mut all = query("rebuild");
        all.include_refuted = true;
        assert_eq!(slugs(&corpus, &all), ["was-the-lockfile"]);
    }

    /// Two memories built to score identically, so only the tiebreak can order
    /// them. BM25 over one-line `tldr` fields ties far more often than BM25 over
    /// prose, and without a deterministic second key `--limit 3` returns a
    /// different three on different runs.
    #[test]
    fn identically_scoring_hits_come_back_in_a_stable_order() {
        let repo = Repo::new();
        for slug in ["zeta-lesson", "alpha-lesson", "middle-lesson"] {
            repo.raw(
                &format!("{slug}.md"),
                "---\ntldr: The rebuild cascade lesson\ngenre: living\n---\nSame body.\n",
            );
        }
        let corpus = repo.load();

        let first = slugs(&corpus, &query("rebuild"));
        assert_eq!(
            first,
            ["alpha-lesson", "middle-lesson", "zeta-lesson"],
            "ties break on slug ascending"
        );
        for _ in 0..5 {
            assert_eq!(
                slugs(&corpus, &query("rebuild")),
                first,
                "and the same order on every run"
            );
        }
    }

    /// `--all` is for refuted memories. It is not a way around the floor: a hit
    /// worse than no answer is worse than no answer either way.
    #[test]
    fn all_does_not_bypass_the_score_floor() {
        let repo = Repo::new();
        repo.memory(
            "rebuild-cascade",
            "prior: 0.0\ngenre: historical\nvalidated:\n  - at: 2016-01-01T00:00:00Z\n    \
             by: t\n    how: c\n    ok: true\n",
            "An env var holding a store path makes every dependent derivation rebuild.\n",
        );
        let corpus = repo.load();
        let mut all = query("rebuild");
        all.include_refuted = true;
        assert!(
            slugs(&corpus, &all).is_empty(),
            "the floor applies with --all too"
        );
    }

    #[test]
    fn a_query_matching_nothing_returns_no_hits() {
        let repo = corpus();
        assert!(
            slugs(&repo.load(), &query("kubernetes ingress certificates")).is_empty(),
            "no answer is the honest answer"
        );
    }

    /// The floor, watched doing its job: the same corpus and query, with the one
    /// matching memory made as weak as the format allows (`prior: 0`, a
    /// down-ranked genre, and a validation old enough to hit the age floor). Its
    /// multipliers fall to 0.5 * 0.5 * 0.3, and what is left is worse than no
    /// answer, so `search` says nothing rather than handing back the least-bad
    /// match.
    #[test]
    fn a_hit_below_the_score_floor_is_dropped_rather_than_returned() {
        let strong = Repo::new();
        strong.memory(
            "rebuild-cascade",
            "prior: 1.0\nvalidated_today",
            "An env var holding a store path makes every dependent derivation rebuild.\n",
        );
        assert_eq!(
            slugs(&strong.load(), &query("rebuild")),
            ["rebuild-cascade"],
            "a confident, current memory clears the floor"
        );

        let weak = Repo::new();
        weak.memory(
            "rebuild-cascade",
            "prior: 0.0\ngenre: historical\nvalidated:\n  - at: 2016-01-01T00:00:00Z\n    \
             by: t\n    how: c\n    ok: true\n",
            "An env var holding a store path makes every dependent derivation rebuild.\n",
        );
        assert!(
            slugs(&weak.load(), &query("rebuild")).is_empty(),
            "the same match, worth 7.5% of its BM25, is worse than an empty result"
        );
    }
}
