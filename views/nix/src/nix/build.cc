#include "nix/cmd/command.hh"
#include "nix/cmd/installable-derived-path.hh"
#include "nix/main/common-args.hh"
#include "nix/main/shared.hh"
#include "nix/store/store-api.hh"
#include "nix/store/local-fs-store.hh"
#include "nix/store/outputs-spec.hh"
#include "nix/expr/eval.hh"
#include "nix/expr/rust-eval-refusal.hh"
#include "nix/expr/shadow-census.hh"
#include "rust-eval-session.hh"

#include <algorithm>
#include <iostream>

#include <nlohmann/json.hpp>

using namespace nix;

/* This serialization code is diferent from the canonical (single)
   derived path serialization because:

   - It looks up output paths where possible

   - It includes the store dir in store paths

   We might want to replace it with the canonical format at some point,
   but that would be a breaking change (to a still-experimental but
   widely-used command, so that isn't being done at this time just yet.
 */

static nlohmann::json toJSON(Store & store, const SingleDerivedPath::Opaque & o)
{
    return store.printStorePath(o.path);
}

static nlohmann::json toJSON(Store & store, const SingleDerivedPath & sdp);
static nlohmann::json toJSON(Store & store, const DerivedPath & dp);

static nlohmann::json toJSON(Store & store, const SingleDerivedPath::Built & sdpb)
{
    nlohmann::json res;
    res["drvPath"] = toJSON(store, *sdpb.drvPath);
    // Fallback for the input-addressed derivation case: We expect to always be
    // able to print the output paths, so let’s do it
    // FIXME try-resolve on drvPath
    const auto outputMap = store.queryPartialDerivationOutputMap(resolveDerivedPath(store, *sdpb.drvPath));
    res["output"] = sdpb.output;
    auto outputPathIter = outputMap.find(sdpb.output);
    if (outputPathIter == outputMap.end())
        res["outputPath"] = nullptr;
    else if (std::optional p = outputPathIter->second)
        res["outputPath"] = store.printStorePath(*p);
    else
        res["outputPath"] = nullptr;
    return res;
}

static nlohmann::json toJSON(Store & store, const DerivedPath::Built & dpb)
{
    nlohmann::json res;
    res["drvPath"] = toJSON(store, *dpb.drvPath);
    // Fallback for the input-addressed derivation case: We expect to always be
    // able to print the output paths, so let’s do it
    // FIXME try-resolve on drvPath
    const auto outputMap = store.queryPartialDerivationOutputMap(resolveDerivedPath(store, *dpb.drvPath));
    for (const auto & [output, outputPathOpt] : outputMap) {
        if (!dpb.outputs.contains(output))
            continue;
        if (outputPathOpt)
            res["outputs"][output] = store.printStorePath(*outputPathOpt);
        else
            res["outputs"][output] = nullptr;
    }
    return res;
}

static nlohmann::json toJSON(Store & store, const SingleDerivedPath & sdp)
{
    return std::visit([&](const auto & buildable) { return toJSON(store, buildable); }, sdp.raw());
}

static nlohmann::json toJSON(Store & store, const DerivedPath & dp)
{
    return std::visit([&](const auto & buildable) { return toJSON(store, buildable); }, dp.raw());
}

static nlohmann::json derivedPathsToJSON(const DerivedPaths & paths, Store & store)
{
    auto res = nlohmann::json::array();
    for (auto & t : paths) {
        res.push_back(toJSON(store, t));
    }
    return res;
}

static nlohmann::json
builtPathsWithResultToJSON(const std::vector<BuiltPathWithResult> & buildables, const Store & store)
{
    auto res = nlohmann::json::array();
    for (auto & b : buildables) {
        auto j = b.path.toJSON(store);
        if (b.result) {
            if (b.result->startTime)
                j["startTime"] = b.result->startTime;
            if (b.result->stopTime)
                j["stopTime"] = b.result->stopTime;
            if (b.result->cpuUser)
                j["cpuUser"] = ((double) b.result->cpuUser->count()) / 1000000;
            if (b.result->cpuSystem)
                j["cpuSystem"] = ((double) b.result->cpuSystem->count()) / 1000000;
        }
        res.push_back(j);
    }
    return res;
}

struct CmdBuild : InstallablesCommand, MixOutLinkByDefault, MixDryRun, MixJSON, MixProfile
{
    bool printOutputPaths = false;
    BuildMode buildMode = bmNormal;

    CmdBuild()
    {
        addFlag({
            .longName = "print-out-paths",
            .description = "Print the resulting output paths",
            .handler = {&printOutputPaths, true},
        });

        addFlag({
            .longName = "rebuild",
            .description = "Rebuild an already built package and compare the result to the existing store paths.",
            .handler = {&buildMode, bmCheck},
        });
    }

    std::string description() override
    {
        return "build a derivation or fetch a store path";
    }

    std::string doc() override
    {
        return
#include "build.md"
            ;
    }

    /**
     * Intercept before the installables are parsed.
     *
     * `parseInstallables` evaluates the `--expr`/`--file` source with the C++
     * evaluator on its way to building an `InstallableAttrPath`, and with the
     * choke point in `EvalState::eval` that is where `nix build` refused:
     * `command-unsupported`, detail `nix build`, measured on dev-compute-6
     * against a two-line derivation (ENG-12799). Routing has to happen here, above that
     * call, or not at all.
     */
    void run(ref<Store> store, std::vector<std::string> && rawInstallables) override
    {
        // Read the setting rather than building an EvalState to ask it, for
        // the reason `nix eval` does: `rustSourceOf` turns `pure-eval` off for
        // `--file`, and a state built before that has already captured the
        // restricted accessor.
        auto backend = evalSettings.evalBackend.get();
        if (backend == "shadow") {
            /* Shadow serves the C++ arm and takes the ordinary path
               unchanged; the comparison happens in `run(store, installables)`
               once that arm has produced an answer, and is built out of that
               answer rather than out of a second evaluation.

               The raw strings are kept because they are about to be moved
               from and the shadow arm needs them: `--expr`/`--file` turns
               them into an `InstallableAttrPath`, which keeps its source and
               its value private, so the attribute path the user asked for is
               not recoverable from the parsed installable.

               Counted here only when the served arm never got as far as an
               answer. That case is why this whole branch exists: `nix build`
               previously reported `attempts: 0` *and* every skip counter 0,
               which is a census that cannot tell "nothing to compare" from
               "this command was never wired up". */
            shadowRawInstallables = rawInstallables;
            /* At least one, even when the list is empty: `applyDefaultInstallables`
               fills in `.` before this point, so an empty list still means one
               evaluation, and counting zero skips for it would put the silent
               hole back. */
            auto expected = std::max<size_t>(1, rawInstallables.size());
            try {
                InstallablesCommand::run(store, std::move(rawInstallables));
            } catch (Error &) {
                for (size_t n = shadowAccounted; n < expected; n++)
                    ShadowCensus::skipped(ShadowSkip::CppFailedBeforeEval);
                shadowAccounted = expected;
                throw;
            }
            return;
        }
        if (backend != "rust") {
            InstallablesCommand::run(store, std::move(rawInstallables));
            return;
        }

        auto src = rustSourceOf(*this);
        auto state = getEvalState();

        Installables installables;
        for (auto & s : rawInstallables) {
            auto [prefix, extendedOutputsSpec] = ExtendedOutputsSpec::parse(s);
            // `foo^out` and `foo^*`. Refused rather than served, because the
            // spec has to be reconciled with `meta.outputsToInstall` and the
            // `Explicit` case is a second reduction rule; the default case is
            // what a build ordinarily asks for.
            if (!std::get_if<ExtendedOutputsSpec::Default>(&extendedOutputsSpec.raw))
                refuse(refusalTokens::outputSelection, "output selection ('%s')", s);
            // Per installable, because each positional argument is its own
            // flake reference when there is no `--file`, with its own lock.
            for (auto & drv : rustEvalDerivations(*state, rustEvaluandOf(*this, state, src, prefix)))
                // Everything past this line is store work and stays cppnix's:
                // the derivation is named, and realising it, querying its
                // outputs and building it are libstore's job exactly as they
                // are for the C++ backend. `InstallableDerivedPath` is the
                // existing class for "a derivation I already know the path
                // of", so there is no second installable to keep in step.
                installables.push_back(
                    make_ref<InstallableDerivedPath>(
                        store,
                        DerivedPath::Built{
                            .drvPath = makeConstantStorePathRef(drv.drvPath),
                            .outputs = OutputsSpec::Names{drv.outputs},
                        }));
        }

        run(store, std::move(installables));
    }

    /**
     * The raw installable strings, kept for the shadow arm. Empty unless
     * `eval-backend = shadow`.
     */
    std::vector<std::string> shadowRawInstallables;

    /// How many of this command's installables the census has heard about.
    size_t shadowAccounted = 0;

    /**
     * What the shadow arm evaluates for one installable, or nothing when it
     * cannot be described (a named skip is counted either way).
     *
     * Two sources, because `nix build` has two: a positional argument, which
     * is a flake and is described from the lock the served arm already
     * computed, and `--expr`/`--file`, whose source this reads with the same
     * reader the served `rust` backend uses. One reader, so the two arms
     * cannot end up evaluating two different programs while reporting a
     * divergence -- the one bug a comparison harness must not have.
     */
    std::optional<RustEvaluand> shadowEvaluandFor(EvalState & state, Installable & installable, size_t n)
    {
        if (!file && !expr)
            return shadowEvaluandOfInstallable(state, installable);

        auto source = rustReadSource(*this);
        if (!source) {
            // A `--file` that is a flake ref or a lookup path: resolving it
            // would run the C++ evaluator and credit this backend with an
            // answer it did not produce.
            ShadowCensus::skipped(ShadowSkip::UnservableShape);
            return std::nullopt;
        }
        auto raw = n < shadowRawInstallables.size() ? shadowRawInstallables[n] : std::string(".");
        auto [prefix, extendedOutputsSpec] = ExtendedOutputsSpec::parse(raw);
        if (!std::get_if<ExtendedOutputsSpec::Default>(&extendedOutputsSpec.raw)) {
            ShadowCensus::skipped(ShadowSkip::UnservableShape);
            return std::nullopt;
        }
        return shadowSubjectOfSource(*source, prefix == "." ? std::string() : std::string(prefix), RustRender::Plain)
            .evaluand;
    }

    /**
     * Compare each installable's derivation against the Rust arm.
     *
     * `cppAnswers` is what the served arm *already computed*, one entry per
     * installable, and that is the load-bearing part: asking the installables
     * for their derived paths a second time would be a second full C++
     * evaluation whenever the eval cache is off, which is every flake in a
     * dirty checkout -- which is every `darwin-rebuild`. Shadow is supposed
     * to cost one extra evaluation, not two.
     *
     * Runs after the served arm rather than before it, so that nothing here
     * can reorder, duplicate or delay a single byte the user sees. The price
     * is that on a real build the comparison happens once the build is done;
     * the deadline in `rustEvalShadow` is what keeps that bounded.
     */
    void shadowInstallables(
        ref<Store> store, const Installables & installables, const std::vector<DerivedPaths> & cppAnswers)
    {
        if (evalSettings.evalBackend.get() != "shadow")
            return;
        try {
            /* Built on first need rather than up front. A command given only
               store paths never evaluates anything, and constructing an
               evaluator afterwards purely to decide there was nothing to
               compare is work the served arm did not do. */
            std::shared_ptr<EvalState> state;
            for (size_t n = 0; n < installables.size(); n++) {
                if (n >= shadowAccounted)
                    shadowAccounted = n + 1;
                if (n >= cppAnswers.size()) {
                    // The served arm produced no entry for this installable at
                    // all, which is still an evaluation nobody compared.
                    ShadowCensus::skipped(ShadowSkip::CppAnswerShape);
                    continue;
                }
                auto text = shadowDerivationText(*store, cppAnswers[n]);
                if (!text)
                    continue; // counted `cpp-answer-shape`, by name, inside
                if (!state)
                    state = getEvalState().get_ptr();
                auto evaluand = shadowEvaluandFor(*state, *installables[n], n);
                if (!evaluand)
                    continue; // counted, by name, inside
                ShadowSubject subject;
                subject.evaluand = std::move(*evaluand);
                subject.question = ShadowQuestion::Derivation;
                subject.what = installables[n]->what();
                rustEvalShadow(*state, subject, ShadowCppOutcome{true, *text, "", ""});
            }
        } catch (std::exception & e) {
            /* The shadow machinery itself failed -- building an evaluator, in
               practice, since nothing below it throws. Said out loud and
               counted, never raised: the user has been served. */
            ShadowCensus::skipped(ShadowSkip::CppFailedBeforeEval);
            std::cerr << "<4>rust-eval shadow: nix build could not compare: " << e.what() << "\n";
            std::cerr.flush();
        }
    }

    void run(ref<Store> store, Installables && installables) override
    {
        if (dryRun) {
            std::vector<DerivedPath> pathsToBuild;
            /* Grouped by installable as well as flattened. The flat list is
               what `printMissing` and the JSON document take, exactly as
               before; the grouping comes from the same single pass, so the
               shadow arm gets the served arm's answer without asking for it
               again. */
            std::vector<DerivedPaths> perInstallable;

            for (auto & i : installables) {
                DerivedPaths mine;
                for (auto & b : i->toDerivedPaths())
                    mine.push_back(b.path);
                for (auto & p : mine)
                    pathsToBuild.push_back(p);
                perInstallable.push_back(std::move(mine));
            }

            printMissing(store, pathsToBuild, lvlError);

            if (json)
                printJSON(derivedPathsToJSON(pathsToBuild, *store));

            shadowInstallables(store, installables, perInstallable);

            return;
        }

        /* `build2` rather than `build`, which is `build2` with the installable
           of each result dropped (`installables.cc`). Keeping it is what lets
           the shadow arm read the served arm's answer per installable off the
           build results instead of evaluating everything a second time.
           `buildables` below is assembled exactly as `build` assembles it. */
        std::vector<BuiltPathWithResult> buildables;
        std::vector<DerivedPaths> perInstallable(installables.size());
        {
            std::map<const Installable *, size_t> indexOf;
            for (size_t n = 0; n < installables.size(); n++)
                indexOf.emplace(&*installables[n], n);
            for (auto & [installable, built] : Installable::build2(
                     getEvalStore(), store, Realise::Outputs, installables, repair ? bmRepair : buildMode)) {
                if (auto it = indexOf.find(&*installable); it != indexOf.end())
                    if (auto derived = shadowDerivedPathOf(built.path))
                        perInstallable[it->second].push_back(std::move(*derived));
                buildables.push_back(built);
            }
        }

        if (json)
            logger->cout("%s", builtPathsWithResultToJSON(buildables, *store).dump());

        createOutLinksMaybe(buildables, store);

        if (printOutputPaths) {
            logger->stop();
            for (auto & buildable : buildables) {
                std::visit(
                    overloaded{
                        [&](const BuiltPath::Opaque & bo) { logger->cout(store->printStorePath(bo.path)); },
                        [&](const BuiltPath::Built & bfd) {
                            for (auto & output : bfd.outputs) {
                                logger->cout(store->printStorePath(output.second));
                            }
                        },
                    },
                    buildable.path.raw());
            }
        }

        BuiltPaths buildables2;
        for (auto & b : buildables)
            buildables2.push_back(b.path);
        updateProfile(*store, buildables2);

        shadowInstallables(store, installables, perInstallable);
    }

    /**
     * The derivation a realised path came from, as a `DerivedPath`.
     *
     * A built path knows more than a derived path does -- it carries the
     * output store paths the build produced -- so this discards back down to
     * the drvPath and the output *names*, which is the pair the two arms are
     * compared on. `nullopt` for an `Opaque` path, which is a store path the
     * user named and nothing evaluated.
     */
    static std::optional<DerivedPath> shadowDerivedPathOf(const BuiltPath & path)
    {
        auto * built = std::get_if<BuiltPath::Built>(&path.raw());
        if (!built)
            return std::nullopt;
        std::set<std::string, std::less<>> names;
        for (auto & [name, _] : built->outputs)
            names.insert(name);
        if (names.empty())
            // `OutputsSpec::Names` asserts non-empty, and a build that
            // produced no output is not an answer to compare against.
            return std::nullopt;
        return DerivedPath::Built{
            .drvPath = make_ref<SingleDerivedPath>(built->drvPath->discardOutputPath()),
            .outputs = OutputsSpec::Names{std::move(names)},
        };
    }
};

static auto rCmdBuild = registerCommand<CmdBuild>("build");
