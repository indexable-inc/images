#pragma once
///@file

#include <cstdint>
#include <map>
#include <string>
#include <string_view>
#include <vector>

namespace nix {

/**
 * What one shadow evaluation concluded.
 *
 * Every attempt reaches exactly one of these, and the counts are held to
 * that: `ShadowCensus::unaccounted()` is `attempts` minus the sum, so a
 * shadow that died mid-call leaves a visible hole rather than a quiet zero.
 * That is the whole reason attempts are counted *before* the Rust arm is
 * entered instead of after it returns.
 */
enum class ShadowVerdict {
    /// Both arms produced a value and the rendered bytes are identical.
    Agreed,
    /// Both arms failed with the same error class *and* the same message.
    AgreedFailure,
    /// Both arms failed with the same error class, wording apart. Counted
    /// apart from `Mismatched` on purpose: the parity bar (CLAUDE.md) puts
    /// error wording in tier 2, where functional equivalence suffices, so
    /// folding these into the mismatch count would put presentation in the
    /// number the default flip is decided on.
    AgreedFailureTextDiffers,
    /// The Rust arm refused. Not a failure under shadow -- the C++ answer is
    /// served either way -- and the token says which construct to write next.
    Refused,
    /// The two arms disagree about the value, or about whether there is one.
    Mismatched,
    /// The Rust arm threw something that is neither a refusal nor a nix
    /// error. Its own verdict because "the backend broke" and "the backend
    /// disagrees" call for different work.
    Crashed,
    /// The attempt ran past what was left of `eval-shadow-budget` and was
    /// stopped part-way through, so the two arms were never compared.
    ///
    /// A verdict rather than a skip, and the distinction is not pedantry: a
    /// skip is decided before `attempt()` is counted, and this is decided
    /// after. Recording it as a skip would leave `attempts` permanently ahead
    /// of the verdict sum and make `unaccounted()` -- the one signal that a
    /// shadow died mid-call -- read non-zero on every ordinary budget cutoff.
    ///
    /// Never a divergence. The Rust arm here failed because this harness
    /// stopped it, so reporting it as `rust-failed-cpp-succeeded` would fill
    /// the histogram with findings about the budget.
    TimedOut,
};

/**
 * Why an evaluation was not shadowed at all.
 *
 * A skip is not an attempt and is never counted as one. Kept because a
 * shadow run reporting `attempts: 0` has two very different readings --
 * nothing evaluated, or everything was skipped -- and only the skip counts
 * tell them apart.
 */
enum class ShadowSkip {
    /// Already inside a shadow evaluation. Bounds the overhead at one extra
    /// evaluation per user evaluation rather than letting it compound.
    Reentrant,
    /// The cumulative time budget for the process is spent.
    Budget,
    /// The C++ arm failed before it evaluated anything -- a bad flag, an
    /// installable that would not parse -- so there is no evaluation to
    /// compare. Its own reason rather than `UnservableShape`, which is about
    /// what the Rust arm cannot serve; this one is about the C++ arm never
    /// getting there, and reading the two as one would hide a command that
    /// started failing early.
    CppFailedBeforeEval,
    /// The invocation is a shape the Rust arm has no path to at all
    /// (`--apply`, `--write-to`, an expression on stdin, `foo^out`), so
    /// running it would count a refusal the census already has from the
    /// `rust` backend and tell nobody anything new.
    ///
    /// This used to cover a flake installable too, which is what made
    /// `nix build` and `nix eval .#attr` report `attempts: 0` for ever. A
    /// flake is now described and attempted; what remains here is genuinely
    /// shapeless.
    UnservableShape,
    /// The positional argument resolved to something that is not a Nix
    /// expression this can describe -- a store path, a derived path, an
    /// installable class with no source behind it.
    ///
    /// Apart from `UnservableShape` because that one is about a flag the user
    /// passed and this one is about what the argument turned out to name, and
    /// a run whose coverage is limited by store-path arguments needs to say
    /// so rather than look like a run full of `--apply`.
    NonValueInstallable,
    /// The argument is a flake, and the evaluand for it could not be built:
    /// the read-set tracker is on (whose recording thunks cannot be
    /// serialised into the overrides document), the lock is not available, or
    /// serialising the lock failed.
    ///
    /// The most important skip to keep separate. It is the one that says "a
    /// flake reached here and was still not compared", which is exactly the
    /// blindness this vocabulary exists to make visible.
    FlakeUnservable,
    /// The C++ arm answered in a shape there is nothing to compare against:
    /// a derivation whose `drvPath` is itself built rather than a plain store
    /// path, or an output set spelled `*` rather than by name.
    ///
    /// About the served arm rather than the Rust one, which is why it is not
    /// `UnservableShape`: nothing here says the Rust backend is missing
    /// anything.
    CppAnswerShape,
    /// This binary has no Rust evaluator linked in (`-Dnix:rust-eval` off),
    /// so there is no second arm to run.
    ///
    /// Named rather than folded into `UnservableShape` because the two call
    /// for opposite work -- one is a gap in the backend, the other is a
    /// build that cannot measure gaps at all -- and reading a whole run of
    /// these as "shapes we do not cover" is how a harness comes to report
    /// green about a binary that compared nothing. See CLAUDE.md, "A setting
    /// is not a capability".
    BackendAbsent,
};

/**
 * Counts of shadow outcomes, and the divergences themselves, for the process.
 *
 * Sibling of `RefusalCensus` and deliberately shaped like it: process-wide
 * rather than per-`EvalState`, one accounting path, and a `<4>`-prefixed
 * journal line at the moment of the event so the record survives a process
 * that dies later.
 *
 * The difference from `RefusalCensus` is which channel is authoritative.
 * A refusal in production is fatal, so its stats block is usually never
 * printed and the journal is the census. Under shadow a refusal is caught and
 * the process lives, so the stats block *is* readable and is the intended
 * view -- which is what "size the histogram for shadow" in the ENG-12546
 * part 2 handoff meant.
 */
struct ShadowCensus
{
    /**
     * One shadow evaluation is about to start.
     *
     * Called before the Rust arm is entered, never after it returns. The
     * ordering is the guard: a call that never comes back leaves
     * `attempts` ahead of the verdicts and `unaccounted()` non-zero.
     */
    static void attempt();

    /**
     * Record what an attempt concluded. `token` is the refusal token for
     * `Refused` and empty otherwise.
     */
    static void record(ShadowVerdict verdict, std::string_view token = {});

    /**
     * Record a divergence: count it, write the journal line, and keep it for
     * the stats block.
     *
     * `kind` is one of `shadowKinds` below. `id` is stable across runs, so
     * the same divergence from two machines groups into one row.
     */
    static void
    diverged(std::string_view kind, const std::string & id, const std::string & origin, const std::string & detail);

    /// An evaluation that was not shadowed, and why.
    static void skipped(ShadowSkip why);

    /// Add to the time spent inside the Rust arm, for the budget and the
    /// stats block.
    static void spent(uint64_t micros);

    /// Whether the cumulative budget is already spent. Zero budget means no
    /// limit.
    static bool budgetExhausted(uint64_t budgetMicros);

    static uint64_t attempts();
    static uint64_t micros();

    /// Counts by verdict, every verdict present so a reader gets a
    /// denominator rather than only what happened to occur.
    static std::map<std::string, uint64_t> verdicts();

    /// Counts by refusal token, for the `Refused` verdicts only.
    static std::map<std::string, uint64_t> refusalTokens();

    /// Counts by skip reason, every reason present.
    static std::map<std::string, uint64_t> skips();

    /// Counts by divergence kind, every kind in `shadowKinds` present.
    static std::map<std::string, uint64_t> divergenceKinds();

    /**
     * `attempts` minus the sum of the verdicts.
     *
     * Non-zero means an attempt reached no conclusion, which is what a Rust
     * arm that died mid-call looks like. Reported rather than asserted: the
     * process that would trip the assertion is exactly the one that cannot
     * run it.
     */
    static uint64_t unaccounted();

    /// One line per distinct divergence, for the stats block. Bounded; see
    /// `maxRememberedDivergences`.
    struct Divergence
    {
        std::string kind;
        std::string id;
        std::string origin;
        std::string detail;
        uint64_t count = 0;
    };

    static std::vector<Divergence> divergences();

    /**
     * How many distinct divergences are kept in memory for the stats block.
     *
     * Repeats of one already-seen divergence only bump its count, so this
     * bounds distinct kinds, not volume. Past the cap the counts and the
     * journal lines keep going and only the in-memory list stops growing:
     * losing the last few examples is a much smaller harm than a report that
     * cannot be printed.
     */
    static constexpr size_t maxRememberedDivergences = 64;
};

/**
 * The stable vocabulary of divergence kinds.
 *
 * Spelled as constants for the reason the refusal tokens are: a literal at
 * the report site mints a category on a typo, and a histogram row nobody can
 * explain is worse than a missing one. `allShadowKinds()` gives the
 * denominator.
 */
namespace shadowKinds {
/// Both arms produced a value and the values differ.
constexpr std::string_view valueMismatch = "value-mismatch";
/// Both failed, with different error classes *and* different messages.
constexpr std::string_view errorClassMismatch = "error-class-mismatch";
/// Both failed with the identical message and different classes.
///
/// Its own row because the cause is systemic and is not in the evaluator:
/// the bridge maps a Rust status to an exception type, and the mapping is
/// coarser than cppnix's hierarchy, so an abort, a type error and a stack
/// overflow all arrive as `EvalError` carrying cppnix's own words. Folded
/// into `error-class-mismatch` this was 53 of 79 rows on the lang corpus and
/// buried the four that are about the evaluator.
constexpr std::string_view errorClassLost = "error-class-lost";
/// Both failed with the same class, different wording. Tier 2, reported so
/// somebody can look, and not counted against the flip.
constexpr std::string_view errorTextMismatch = "error-text-mismatch";
/// The C++ arm answered and the Rust arm failed for a reason that is not a
/// refusal.
constexpr std::string_view rustFailed = "rust-failed-cpp-succeeded";
/// The Rust arm answered and the C++ arm failed. The rarer and more
/// interesting direction: the Rust backend accepted a program cppnix rejects.
constexpr std::string_view cppFailed = "cpp-failed-rust-succeeded";
/// The Rust arm threw something that is not a nix error at all.
constexpr std::string_view rustCrashed = "rust-crashed";
} // namespace shadowKinds

const std::vector<std::string_view> & allShadowKinds();

/// The stable name of a verdict, for the stats block and the journal.
std::string_view shadowVerdictName(ShadowVerdict verdict);

/// Every verdict, so the histogram has a denominator.
const std::vector<ShadowVerdict> & allShadowVerdicts();

/// The stable name of a skip reason.
std::string_view shadowSkipName(ShadowSkip why);

/// Every skip reason, for the same reason.
const std::vector<ShadowSkip> & allShadowSkips();

} // namespace nix
