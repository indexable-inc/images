#pragma once
///@file

#include <cstdint>
#include <map>
#include <string>

namespace nix {

/**
 * Where a Rust-backend evaluation's time went, for the `NIX_SHOW_STATS`
 * block.
 *
 * Deliberately shaped like `RefusalCensus`, and for the same reason: the
 * numbers are produced on the far side of the C ABI, in a translation unit
 * that `libexpr` does not link, so the bridge pushes them here and
 * `EvalState::printStatistics` reads them. One accounting path feeding one
 * derivation in the stats block.
 *
 * # Why the numbers come from over there at all
 *
 * The Rust VM is a flat trampoline, so a sampling profiler attributes an
 * entire evaluation to one inlined frame and nothing below it. cppnix's own
 * counters (`nrFunctionCalls`, `nrThunks`) describe the cpp arm and say
 * nothing about the rust one. Anything anybody wants to know about where the
 * rust arm's time goes has to be counted inside it; this is the pipe.
 *
 * # What a missing block means
 *
 * Absent, not zero. A build without the Rust evaluator, or a run that
 * evaluated on the cpp arm, records nothing and the stats block has no
 * `rustEvalPerf` key at all. That is on purpose: a block of zeros would read
 * as "the rust arm did no work" rather than "the rust arm did not run", and
 * this repo has been bitten by that shape often enough to make the
 * distinction structural.
 */
struct EvalPerfCensus
{
    /**
     * Record one evaluation's counters, as the `key=value` line
     * `ixe_perf_snapshot` renders.
     *
     * Parsed rather than passed as a struct so the field names have exactly
     * one spelling, the Rust one. A second definition on this side is how a
     * dashboard and a gate come to disagree about what `questions` means.
     */
    static void record(std::string_view line);

    /**
     * The counters recorded so far, or an empty map if none were. Values are
     * summed across evaluations, so a process that evaluated twice reports
     * the total.
     */
    static std::map<std::string, uint64_t> snapshot();

    /**
     * Whether anything was ever recorded, which is what tells the stats block
     * to emit the key at all. `snapshot().empty()` would conflate "nothing
     * recorded" with "recorded all zeros".
     */
    static bool recorded();
};

} // namespace nix
