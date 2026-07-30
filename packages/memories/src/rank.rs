//! The ranking formula.
//!
//! Every constant here is a placed guess, not a measurement: they are chosen to
//! be defensible and are meant to be tuned once there are enough memories and a
//! handful of queries with known answers. Each one says what it defends against
//! so a later measurement can argue with it.

use crate::model::{Genre, Memory};
use chrono::{DateTime, Utc};

/// Floor of the `prior` multiplier.
///
/// A memory whose author had no confidence at all still ranks on its BM25
/// match, at half weight. Without a floor a `prior: 0` memory could never
/// surface, which makes writing one pointless. Placed guess.
pub const PRIOR_BASE: f64 = 0.5;

/// How much of the multiplier `prior` controls, on top of [`PRIOR_BASE`].
///
/// The two sum to 1.0 so a `prior: 1` memory is unpenalized. Placed guess.
pub const PRIOR_WEIGHT: f64 = 0.5;

/// Multiplier for `historical` and `frozen`.
///
/// They are kept deliberately and must still be findable, so they are ranked
/// down rather than excluded; half weight is enough to lose a tie with a live
/// memory and not enough to hide. Placed guess.
pub const DOWNRANKED_GENRE_FACTOR: f64 = 0.5;

/// Exponential decay constant, in days since the newest `validated.at`.
///
/// Ninety days is about a quarter, the horizon over which a claim about this
/// codebase stops being something anyone has checked. Placed guess awaiting
/// measurement.
pub const AGE_DECAY_DAYS: f64 = 90.0;

/// Floor under the age decay.
///
/// Without it decay reaches zero and an old memory becomes unfindable, which is
/// worse than ranking it low: the harm from an old memory is reading it
/// unflagged, not finding it. Placed guess.
pub const AGE_FACTOR_FLOOR: f64 = 0.3;

/// Age multiplier for a memory nobody has validated yet.
///
/// Decay measures how long since the last confirmation, and a never-confirmed
/// memory has no such interval, so it is not decayed; it already forgoes the
/// reinforcement bonus. The alternative, treating it as maximally stale, would
/// bury every memory in the minutes after it was written.
pub const UNVALIDATED_AGE_FACTOR: f64 = 1.0;

/// Weight on the logarithmic reinforcement term.
///
/// Logarithmic so the second confirmation counts for much more than the tenth:
/// repeated confirmations of the same fact are correlated, and a linear count
/// would let one memory validated in a loop dominate the ranking. Placed guess.
pub const REINFORCEMENT_WEIGHT: f64 = 0.15;

/// Score a hit must reach to be returned at all.
///
/// `search` returns nothing rather than its best of a bad set: a query with no
/// good answer comes back empty, so the caller can say so instead of acting on
/// the least-bad match. The generative-agents formula this borrows from has no
/// such floor, and always-return-something is the one configuration this fleet
/// measured as net-negative.
///
/// The value is low on purpose. BM25 is not normalized, so the same match scores
/// differently in a 3-file corpus than in a 300-file one, and the first thing
/// measured here was a three-memory corpus where a genuinely relevant hit scored
/// 0.46 while a query matching nothing scored 0: a floor of 0.5 emptied a small
/// corpus of its real answers. This cuts the near-zero tail without punishing a
/// repo that has just started writing memories. It is a placed guess, and an
/// absolute floor is the wrong shape for an unnormalized score: a fraction of the
/// top hit would not move with corpus size. Left absolute because the contract
/// names a `MIN_SCORE`.
pub const MIN_SCORE: f64 = 0.1;

/// Seconds in a day, for turning a duration into the decay's units.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// The contract's score:
///
/// ```text
/// score = bm25
///       * (0.5 + 0.5 * prior)
///       * genre_factor
///       * max(0.3, exp(-age_days / 90))
///       * (1 + 0.15 * ln(1 + n_ok))
/// ```
#[must_use]
pub fn score(memory: &Memory, bm25: f64, now: DateTime<Utc>) -> f64 {
    bm25 * prior_factor(memory.prior)
        * genre_factor(memory.genre)
        * age_factor(memory, now)
        * reinforcement_factor(memory.ok_count())
}

#[must_use]
pub const fn prior_factor(prior: f64) -> f64 {
    PRIOR_WEIGHT.mul_add(prior, PRIOR_BASE)
}

#[must_use]
pub const fn genre_factor(genre: Genre) -> f64 {
    match genre {
        Genre::Historical | Genre::Frozen => DOWNRANKED_GENRE_FACTOR,
        Genre::Memory | Genre::Living | Genre::Recipe => 1.0,
    }
}

/// Decay since the newest validation, floored at [`AGE_FACTOR_FLOOR`].
#[must_use]
pub fn age_factor(memory: &Memory, now: DateTime<Utc>) -> f64 {
    let Some(newest) = memory.newest_validation() else {
        return UNVALIDATED_AGE_FACTOR;
    };
    age_factor_for_days(days_between(newest.at_time, now))
}

/// Days from `at` to `now`, never negative. A timestamp in the future is clock
/// skew or a hand-edited file, and letting it produce a negative age would turn
/// the decay into a boost.
#[must_use]
pub fn days_between(at: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    let seconds = (now - at).num_seconds();
    if seconds <= 0 {
        return 0.0;
    }
    // A duration in seconds is far inside f64's exact integer range for any
    // timestamp this format can hold.
    #[expect(
        clippy::cast_precision_loss,
        reason = "second counts stay far below 2^53"
    )]
    let seconds = seconds as f64;
    seconds / SECONDS_PER_DAY
}

#[must_use]
pub fn age_factor_for_days(age_days: f64) -> f64 {
    AGE_FACTOR_FLOOR.max((-age_days / AGE_DECAY_DAYS).exp())
}

/// Logarithmic reinforcement in the number of `ok: true` validations.
#[must_use]
pub fn reinforcement_factor(ok_count: usize) -> f64 {
    // Counts here are directory-sized, far inside f64's exact integer range.
    #[expect(
        clippy::cast_precision_loss,
        reason = "validation counts stay far below 2^53"
    )]
    let ok_count = ok_count as f64;
    REINFORCEMENT_WEIGHT.mul_add(ok_count.ln_1p(), 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_memory;
    use std::path::Path;

    fn memory(frontmatter: &str) -> Memory {
        let contents = format!("---\ntldr: A line\n{frontmatter}---\nBody.\n");
        parse_memory(
            Path::new("/repo/.memories/a-slug.md"),
            Path::new("/repo"),
            &contents,
        )
        .expect("fixture must parse")
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-29T00:00:00Z")
            .expect("fixed clock")
            .with_timezone(&Utc)
    }

    fn validated_block(at: &str, ok: bool) -> String {
        format!("validated:\n  - at: {at}\n    by: t\n    how: c\n    ok: {ok}\n")
    }

    /// The whole formula on a hand-computed fixture. `prior: 0.8` and one
    /// same-day confirmation: 10 * 0.9 * 1.0 * 1.0 * (1 + 0.15 * ln 2).
    #[test]
    fn score_matches_the_formula_on_a_hand_built_fixture() {
        let fixture = memory(&format!(
            "prior: 0.8\n{}",
            validated_block("2026-07-29T00:00:00Z", true)
        ));
        let expected = 10.0 * 0.9 * 0.15_f64.mul_add(2.0_f64.ln(), 1.0);
        let actual = score(&fixture, 10.0, now());
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn historical_genre_halves_the_score() {
        let live = memory("genre: living\n");
        let historical = memory("genre: historical\n");
        let frozen = memory("genre: frozen\n");
        let live_score = score(&live, 10.0, now());
        assert!(
            (score(&historical, 10.0, now()) - live_score * 0.5).abs() < 1e-9,
            "historical must be half of {live_score}"
        );
        assert!(
            (score(&frozen, 10.0, now()) - live_score * 0.5).abs() < 1e-9,
            "frozen must be half of {live_score}"
        );
    }

    #[test]
    fn age_decay_floors_at_the_documented_value_rather_than_reaching_zero() {
        // Ten years of decay: exp(-3650/90) is about 4e-18, so only the floor
        // keeps this memory findable at all.
        let ancient = memory(&validated_block("2016-07-29T00:00:00Z", true));
        let factor = age_factor(&ancient, now());
        assert!(
            (factor - AGE_FACTOR_FLOOR).abs() < 1e-12,
            "expected the {AGE_FACTOR_FLOOR} floor, got {factor}"
        );
        assert!(
            score(&ancient, 10.0, now()) > 0.0,
            "a floored memory still scores above zero"
        );
    }

    #[test]
    fn one_decay_constant_of_age_costs_exactly_one_e_fold() {
        let factor = age_factor_for_days(AGE_DECAY_DAYS);
        assert!(
            (factor - (-1.0_f64).exp()).abs() < 1e-12,
            "expected 1/e at {AGE_DECAY_DAYS} days, got {factor}"
        );
    }

    #[test]
    fn future_validation_timestamps_do_not_boost_the_score() {
        let skewed = memory(&validated_block("2099-01-01T00:00:00Z", true));
        let factor = age_factor(&skewed, now());
        assert!(
            (factor - 1.0).abs() < f64::EPSILON,
            "a future timestamp must cap at 1.0, got {factor}"
        );
    }

    #[test]
    fn reinforcement_is_logarithmic_so_late_confirmations_add_less() {
        let second = reinforcement_factor(2) - reinforcement_factor(1);
        let tenth = reinforcement_factor(10) - reinforcement_factor(9);
        assert!(
            tenth < second,
            "the 10th confirmation added {tenth}, the 2nd added {second}"
        );
        assert!(second > 0.0, "confirmations must still help: {second}");
    }

    #[test]
    fn an_unvalidated_memory_is_not_decayed_but_earns_no_reinforcement() {
        let fresh = memory("");
        assert!((age_factor(&fresh, now()) - UNVALIDATED_AGE_FACTOR).abs() < f64::EPSILON);
        assert!(
            (reinforcement_factor(fresh.ok_count()) - 1.0).abs() < f64::EPSILON,
            "no confirmations means no bonus"
        );
    }
}
