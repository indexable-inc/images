#include "nix/cmd/command.hh"
#include "nix/cmd/installable-value.hh"
#include "nix/main/common-args.hh"
#include "nix/main/shared.hh"
#include "nix/store/store-api.hh"
#include "nix/expr/eval.hh"
#include "nix/expr/eval-inline.hh"
#include "nix/expr/eval-settings.hh"
#include "nix/fetchers/input-cache.hh"
#include "nix/cmd/installable-flake.hh"
#include "nix/flake/flake.hh"
#include "nix/fetchers/git-utils.hh"
#include "nix/util/posix-source-accessor.hh"
#include "nix/expr/eval-readset.hh"
#include "nix/expr/eval-retain.hh"

#include <nlohmann/json.hpp>

#include "nix/util/environment-variables.hh"

#include <ctime>
#include <iostream>

using namespace nix;

namespace {

uint64_t readClockNs(clockid_t clock)
{
    struct timespec ts;
    if (clock_gettime(clock, &ts) != 0)
        throw SysError("reading clock");
    return uint64_t(ts.tv_sec) * 1000000000ull + uint64_t(ts.tv_nsec);
}

/**
 * What one request cost, and what it answered.
 *
 * The answer is here because the point of the whole exercise is that a warm
 * evaluation returns what a cold one would. A report of the times alone
 * cannot distinguish reuse from a wrong answer served quickly, which is the
 * failure mode this feature exists to avoid.
 */
struct RequestResult
{
    std::string installable;
    uint64_t wallNs, cpuNs, thunks;
    size_t inputsEvicted;
    std::string value;
    /** Which tree the root flake actually resolved to this time round. */
    std::string lockedRef;
    std::string flakePath;
    /**
     * How many files this request asked for, and how many were already
     * evaluated. The pair says whether reusing evaluated files is where a
     * warm request's time goes, which a duration on its own cannot.
     */
    uint64_t evalFileCalls, evalFilePathHits;
    /**
     * What retention did for this request: how many derivation results
     * were spliced from the previous graph, how many entries the dirty
     * walk condemned, and whether this request became the new baseline.
     * All zero when retention is off.
     */
    uint64_t spliced = 0, dirtySeeds = 0, dirtyEntries = 0, movedTrees = 0, dirtyRefused = 0;
    bool baselineReplaced = false;
};

} // namespace

struct CmdEvalPersistent : SourceExprCommand, MixReadOnlyOption
{
    std::vector<std::string> installables_;
    bool interactive = false;
    bool retain = false;
    std::string evictMode = "unlocked";
    /**
     * The last complete graph per installable. A request that spliced
     * anything holds entries whose inputs were never observed, so only a
     * splice-free request may become the baseline later requests are
     * validated against.
     */
    std::map<std::string, std::shared_ptr<RetainedEval>> baselines;

    CmdEvalPersistent()
    {
        expectArgs({
            .label = "installables",
            .handler = {&installables_},
            .completer = getCompleteInstallable(),
        });

        addFlag({
            .longName = "interactive",
            .description =
                "Read one installable per line from standard input instead of taking them as arguments, and "
                "evaluate each as it arrives. This is what lets a caller edit the tree between two evaluations "
                "of the same attribute, which is the case the persistent evaluator exists for.",
            .handler = {&interactive, true},
        });

        addFlag({
            .longName = "retain",
            .description = "Keep the tracked-entry graph and derivation results of each request, and answer the "
                           "clean derivations of a later request from them instead of re-forcing their attributes. "
                           "Value flows into a derivation are tracked at file granularity through the provenance "
                           "side table; what escapes it is an integer or boolean influencing a derivation without "
                           "any same-file string beside it, and influence through control flow alone. Compare "
                           "answers against a fresh process before trusting an unmeasured edit class.",
            .handler = {&retain, true},
        });

        addFlag({
            .longName = "evict",
            .description =
                "Which fetched inputs to drop before each request: `unlocked` (the default, and the only "
                "sound choice), `all` (drop everything, which throws away the reuse this command exists for), "
                "or `none` (keep everything, which serves a later request the earlier one's tree). The last "
                "two exist to locate where a stale answer comes from.",
            .labels = {"mode"},
            .handler = {&evictMode},
        });
    }

    std::string description() override
    {
        return "evaluate several Nix expressions in one long lived evaluator";
    }

    std::string doc() override
    {
        return
#include "eval-persistent.md"
            ;
    }

    Category category() override
    {
        return catSecondary;
    }

    RequestResult evaluateOne(ref<Store> store, ref<EvalState> state, const std::string & request)
    {
        /* Mutable trees have to be refetched or this evaluation answers with
           the previous one's bytes. See `InputCache::evictUnlocked`. */
        /* Both caches have to go together. Evicting the input cache alone
           makes the fetcher run again, and the fetcher then reads a working
           directory state cached for the lifetime of the process and
           concludes nothing changed. */
        size_t evicted = 0;
        if (evictMode != "none") {
            GitRepo::clearCachedWorkdirInfo();
            /* Same reason, second cache: the posix accessor memoises lstat
               results in a process-global map, so without this the fetcher
               re-runs, re-walks, and still reads the previous request's
               mtimes -- the fetch is not skipped, the mtime is stale. */
            PosixSourceAccessor::clearCache();
        }
        if (evictMode == "unlocked")
            evicted = state->inputCache->evictUnlocked(state->fetchSettings);
        else if (evictMode == "all")
            state->inputCache->clear();
        else if (evictMode != "none")
            throw UsageError("unknown --evict mode '%s'", evictMode);

        std::shared_ptr<RetainedEval> baseline;
        if (retain) {
            /* One tracker per request, so each request leaves a graph of
               its own rather than appending to the last one. The tracker
               asserts it is alone on the thread, so the old one goes
               first. */
            state->readSetTracker.reset();
            state->readSetTracker = std::make_unique<ReadSetTracker>(*state, std::nullopt, true, true);
            if (auto b = baselines.find(request); b != baselines.end()) {
                baseline = b->second;
                baseline->beginRequest();
                /* Diagnostic splice log, append mode, one process-lifetime
                   file. Comparing its produced paths against a fresh-process
                   trace of the same tree names every stale splice. */
                if (auto path = getEnv("NIX_RETAIN_LOG")) {
                    baseline->spliceLog = fopen(path->c_str(), "a");
                    if (baseline->spliceLog)
                        fprintf(baseline->spliceLog, "request %s\n", request.c_str());
                }
            }
            state->retainedPrev = baseline;
        }

        auto wall0 = readClockNs(CLOCK_MONOTONIC);
        auto cpu0 = readClockNs(CLOCK_PROCESS_CPUTIME_ID);
        auto thunks0 = getNrThunks();
        auto evalFileCalls0 = state->nrEvalFileCalls.load();
        auto evalFilePathHits0 = state->nrEvalFilePathHits.load();

        auto installable = parseInstallable(store, request);
        auto v = InstallableValue::require(installable)->toValue(*state).first;
        state->forceValue(*v, noPos);

        NixStringContext context;
        auto value = state->coerceToString(noPos, *v, context, "while showing the result", true, false).toOwned();

        auto wall1 = readClockNs(CLOCK_MONOTONIC);
        auto cpu1 = readClockNs(CLOCK_PROCESS_CPUTIME_ID);

        /* Which tree answered. A warm request that reports the previous
           request's tree has been served stale at the fetch layer, which
           looks identical to real reuse if only the times are reported. */
        std::string lockedRef, flakePath;
        if (auto flake = std::dynamic_pointer_cast<InstallableFlake>(installable.get_ptr())) {
            auto locked = flake->getLockedFlake();
            lockedRef = locked->flake.lockedRef.to_string();
            flakePath = locked->flake.path.to_string();
        }

        RetainedEval::Stats retainStats{};
        bool baselineReplaced = false;
        if (retain) {
            if (baseline) {
                retainStats = baseline->stats;
                if (baseline->spliceLog) {
                    fclose(baseline->spliceLog);
                    baseline->spliceLog = nullptr;
                }
            }
            state->retainedPrev = nullptr;
            auto graph = state->readSetTracker->extractRetained();
            state->readSetTracker.reset();
            /* A request served from live memoization re-enters almost no
               boundary, so its graph describes what re-ran rather than
               the computation, and letting it displace the cold graph
               would leave the next edit nothing to validate against.
               Keep the largest splice-free graph instead. */
            auto current = baselines.find(request);
            if (retainStats.spliced == 0
                && (current == baselines.end() || graph->entries.size() >= current->second->entries.size())) {
                baselines[request] = std::move(graph);
                baselineReplaced = true;
            }
        }

        return RequestResult{
            .installable = request,
            .wallNs = wall1 - wall0,
            .cpuNs = cpu1 - cpu0,
            .thunks = getNrThunks() - thunks0,
            .inputsEvicted = evicted,
            .value = value,
            .lockedRef = lockedRef,
            .flakePath = flakePath,
            .evalFileCalls = state->nrEvalFileCalls.load() - evalFileCalls0,
            .evalFilePathHits = state->nrEvalFilePathHits.load() - evalFilePathHits0,
            .spliced = retainStats.spliced,
            .dirtySeeds = retainStats.dirtySeeds,
            .dirtyEntries = retainStats.dirtyEntries,
            .movedTrees = retainStats.movedTrees,
            .dirtyRefused = retainStats.dirtyRefused,
            .baselineReplaced = baselineReplaced,
        };
    }

    void report(const RequestResult & r)
    {
        nlohmann::json j;
        j["installable"] = r.installable;
        j["wallMs"] = double(r.wallNs) / 1e6;
        j["cpuMs"] = double(r.cpuNs) / 1e6;
        j["thunks"] = r.thunks;
        j["inputsEvicted"] = r.inputsEvicted;
        j["value"] = r.value;
        j["lockedRef"] = r.lockedRef;
        j["flakePath"] = r.flakePath;
        j["evalFileCalls"] = r.evalFileCalls;
        j["evalFilePathHits"] = r.evalFilePathHits;
        j["spliced"] = r.spliced;
        j["dirtySeeds"] = r.dirtySeeds;
        j["dirtyEntries"] = r.dirtyEntries;
        j["movedTrees"] = r.movedTrees;
        j["dirtyRefused"] = r.dirtyRefused;
        j["baselineReplaced"] = r.baselineReplaced;
        std::cout << j.dump() << std::endl;
    }

    void run(ref<Store> store) override
    {
        auto state = getEvalState();

        for (const auto & request : installables_)
            report(evaluateOne(store, state, request));

        if (interactive) {
            std::string line;
            while (std::getline(std::cin, line)) {
                if (line.empty())
                    continue;
                report(evaluateOne(store, state, line));
            }
        }
    }
};

static auto rCmdEvalPersistent = registerCommand<CmdEvalPersistent>("eval-persistent");
