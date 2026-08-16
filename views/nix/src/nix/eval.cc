#include "nix/cmd/command-installable-value.hh"
#include "nix/main/common-args.hh"
#include "nix/main/shared.hh"
#include "nix/store/store-api.hh"
#include "nix/expr/eval.hh"
#include "nix/expr/eval-inline.hh"
#include "nix/expr/value-to-json.hh"
#include "nix/store/outputs-spec.hh"
#include "rust-eval-session.hh"
#include "nix/expr/rust-eval-refusal.hh"
#include "nix/expr/shadow-census.hh"

#include <nlohmann/json.hpp>

using namespace nix;

struct CmdEval : MixJSON, InstallableValueCommand, MixReadOnlyOption
{
    bool raw = false;
    std::optional<std::string> apply;
    std::optional<std::filesystem::path> writeTo;

    CmdEval()
        : InstallableValueCommand()
    {
        addFlag({
            .longName = "raw",
            .description = "Print strings without quotes or escaping.",
            .handler = {&raw, true},
        });

        addFlag({
            .longName = "apply",
            .description = "Apply the function *expr* to each argument.",
            .labels = {"expr"},
            .handler = {&apply},
        });

        addFlag({
            .longName = "write-to",
            .description = "Write a string or attrset of strings to *path*.",
            .labels = {"path"},
            .handler = {&writeTo},
        });
    }

    std::string description() override
    {
        return "evaluate a Nix expression";
    }

    std::string doc() override
    {
        return
#include "eval.md"
            ;
    }

    Category category() override
    {
        return catSecondary;
    }

    /**
     * Intercept before the installable is parsed.
     *
     * `parseInstallables` evaluates the `--expr`/`--file` source with the C++
     * evaluator on its way to building an `InstallableAttrPath`, so by the
     * time `run(store, installable)` is reached the wrong backend has already
     * run -- and with the choke point in `EvalState::eval` it has already
     * refused. Routing has to happen here or not at all.
     */
    void run(ref<Store> store) override
    {
        // Read the setting, do not build an EvalState to ask it. Constructing
        // one here changed the C++ arm's behaviour: `parseInstallables` turns
        // `pure-eval` off when `--file` is given, and an EvalState built
        // before that has already captured the restricted accessor, so
        // `nix eval -f /tmp/x.nix` started failing with "access to absolute
        // path ... is forbidden in restricted mode" on a machine with
        // `pure-eval = true` in nix.conf. The setting is the same one
        // `EvalState`'s constructor reads.
        auto backend = evalSettings.evalBackend.get();
        if (backend == "rust") {
            runWithRustBackend();
            return;
        }
        /* Shadow serves the C++ arm, so it takes the ordinary path and only
           arranges for the comparison at the end of it. Described here rather
           than down in `run(store, installable)`, because this is the only
           place that still knows what the user asked for: by then the
           `--expr`/`--file` source has been turned into an installable and
           cannot be recovered. */
        if (backend != "shadow") {
            InstallableCommand::run(store);
            return;
        }

        shadowSubject = describeShadowSource();
        try {
            InstallableCommand::run(store);
        } catch (Error & e) {
            /* The C++ arm failed, and the two arms failing the same way is
               exactly as much of a comparison as the two agreeing. Skipping
               it would make the census silently blind to every expression
               that does not evaluate, which on a real workload is a large
               share of the interesting ones.

               The state this command already built, not `getEvalState()`:
               the failure may have come from before there was an evaluator,
               and building one while unwinding to report on an evaluation
               that never happened would be inventing a measurement. When
               there is none, the skip says so rather than the run quietly
               reporting one fewer attempt. */
            if (!shadowAccounted) {
                shadowAccounted = true;
                if (shadowSubject && shadowState)
                    rustEvalShadow(
                        *shadowState, *shadowSubject, ShadowCppOutcome{false, "", shadowErrorClass(e), e.message()});
                else
                    /* No subject, or no evaluator: the failure came from
                       before there was an evaluation to compare -- a flake
                       reference that would not resolve, a bad flag -- and
                       building one while unwinding would be inventing a
                       measurement. A flake that got as far as being forced
                       and *then* failed does have a subject, because
                       `run(store, installable)` described it before touching
                       the value, so this is genuinely the early cases. */
                    ShadowCensus::skipped(ShadowSkip::CppFailedBeforeEval);
            }
            throw;
        }
    }

    /**
     * Whether this invocation has been accounted for: attempted, or skipped
     * with a reason.
     *
     * The success path compares where the rendered text is known, and the
     * failure path compares while unwinding. Without this flag an expression
     * that rendered and *then* threw -- a lazy value failing inside
     * `ensureLazyPathsCopied`, say -- would be counted twice, and the second
     * count would carry a verdict about an evaluation that had already
     * agreed.
     *
     * It covers skips as well as attempts, which it did not have to when the
     * only way to reach `run` without a subject was a shape that had already
     * counted itself. An installable's subject is not known until the C++ arm
     * has resolved it, so "no subject yet" and "no subject ever" are now
     * different states, and only this flag tells them apart. Getting that
     * wrong is the failure this whole change is about: an evaluation that is
     * neither attempted nor skipped is one the census cannot see.
     */
    bool shadowAccounted = false;

    /**
     * The evaluator the C++ arm used, kept so a failure can still be compared.
     *
     * Captured inside `run(store, installable)` rather than read from
     * `EvalCommand` (where it is private) and rather than built up front: an
     * `EvalState` constructed before `parseInstallables` has already captured
     * the restricted accessor, and `--file` turns `pure-eval` off after that
     * point, which is how `nix eval -f` came to fail with "access to absolute
     * path is forbidden" on a machine with `pure-eval = true`.
     */
    std::shared_ptr<EvalState> shadowState;

    /// Compare the served C++ answer against the Rust arm, once.
    void shadowAgainst(EvalState & state, const std::string & text)
    {
        if (!shadowSubject || shadowAccounted)
            return;
        shadowAccounted = true;
        rustEvalShadow(state, *shadowSubject, ShadowCppOutcome{true, text, "", ""});
    }

    /**
     * The evaluation to hand the shadow arm, or nothing when there is none.
     *
     * "Nothing" is a skip and not a refusal: under shadow the C++ arm answers
     * whatever this cannot describe, so nobody is denied anything, and
     * counting it as a refusal would inflate the census with rows that say
     * only "shadow was on and this command is not wired", which the `rust`
     * backend already reports far more precisely.
     *
     * The `--expr`/`--file` reading is deliberately the same code
     * `runWithRustBackend` uses. Two copies would let the two arms evaluate
     * two different programs while reporting a divergence, which is the one
     * bug a comparison harness must not have.
     *
     * That reader is `rustReadSource`, in the bridge, because `nix build`
     * needs it too. It reads and decides nothing else: an undescribable
     * source comes back as `nullopt`, and what that *means* is the caller's,
     * which is the whole difference between the two arms here. Under
     * `shadow` it is a skip nobody needs to hear about; under `rust` it is
     * `rustSourceOf`, which refuses by name and also relaxes `pure-eval` for
     * `--file` -- a thing this arm must not do, since here the C++ evaluator
     * is the one serving and its behaviour must not move.
     */
    std::optional<ShadowSubject> describeShadowSource()
    {
        if (apply || writeTo || (file && *file == "-")) {
            ShadowCensus::skipped(ShadowSkip::UnservableShape);
            shadowAccounted = true;
            return std::nullopt;
        }
        auto [prefix, extendedOutputsSpec] = ExtendedOutputsSpec::parse(rawInstallable());
        if (!std::get_if<ExtendedOutputsSpec::Default>(&extendedOutputsSpec.raw)) {
            ShadowCensus::skipped(ShadowSkip::UnservableShape);
            shadowAccounted = true;
            return std::nullopt;
        }
        /* Neither `--expr` nor `--file`: the positional argument is an
           installable, and what it means cannot be settled without an
           evaluator, a store and the registry. Nothing is counted here --
           `describeShadowInstallable` decides one phase later, once the C++
           arm has resolved it. This used to be a `unservable-shape` skip, and
           it is the whole of why `nix eval .#attr` reported `attempts: 0`
           however capable the Rust arm was. */
        if (!file && !expr)
            return std::nullopt;
        auto source = rustReadSource(*this);
        if (!source) {
            ShadowCensus::skipped(ShadowSkip::UnservableShape);
            shadowAccounted = true;
            return std::nullopt;
        }
        return shadowSubjectOfSource(
            *source,
            prefix == "." ? std::string() : std::string(prefix),
            shadowRender(),
            shadowRender() == RustRender::ValuePrinter);
    }

    /// Which bytes this invocation's two arms are compared on.
    RustRender shadowRender() const
    {
        return json ? RustRender::Json : raw ? RustRender::Raw : RustRender::ValuePrinter;
    }

    /**
     * The evaluation behind a positional installable, once the C++ arm has
     * resolved and forced it.
     *
     * Called from `run(store, installable)` and not from `run(ref<Store>)`,
     * because a flake's evaluand is built out of its **locked** flake and the
     * lock only exists after the served arm has asked for the value. Calling
     * it earlier would make this harness do the locking -- fetching inputs
     * and writing `flake.lock` for a measurement -- which is the one thing a
     * shadow must never do.
     */
    void describeShadowInstallable(EvalState & state, Installable & installable)
    {
        if (shadowSubject || shadowAccounted)
            return;
        auto evaluand = shadowEvaluandOfInstallable(state, installable);
        if (!evaluand) {
            // Counted, by name, inside `shadowEvaluandOfInstallable`.
            shadowAccounted = true;
            return;
        }
        ShadowSubject subject;
        subject.evaluand = std::move(*evaluand);
        subject.render = shadowRender();
        subject.nestedFailureIsUnimplemented = subject.render == RustRender::ValuePrinter;
        subject.question = ShadowQuestion::Render;
        subject.what = installable.what();
        shadowSubject = std::move(subject);
    }

    /// The evaluation the shadow arm should run, when there is one. Filled by
    /// `run(ref<Store>)` before the C++ arm starts, read after it finishes.
    std::optional<ShadowSubject> shadowSubject;

    /**
     * `nix eval` served by the Rust bytecode backend.
     *
     * What it covers: `--expr` and `--file` sources, an attribute path with
     * attribute and list-index components, and all three output modes. What
     * it does not is named one refusal at a time, because a user who set
     * `eval-backend = rust` and hit an edge has to learn which edge.
     */
    void runWithRustBackend()
    {
        if (raw && json)
            throw UsageError("--raw and --json are mutually exclusive");
        if (file && expr)
            throw UsageError("'--file' and '--expr' are exclusive");

        if (apply)
            refuse(refusalTokens::apply, "nix eval --apply");
        if (writeTo)
            refuse(refusalTokens::writeTo, "nix eval --write-to");

        // Every source-shape refusal, and the `pure-eval` relaxation, in the
        // one place `nix build` reads them from too. Empty when the command
        // was given an installable instead, which needs an evaluator to
        // resolve and so waits for the line below.
        auto src = rustSourceOf(*this);

        auto state = getEvalState();

        // `foo^out` selects derivation outputs, which needs derivations.
        auto [prefix, extendedOutputsSpec] = ExtendedOutputsSpec::parse(rawInstallable());
        if (!std::get_if<ExtendedOutputsSpec::Default>(&extendedOutputsSpec.raw))
            refuse(refusalTokens::outputSelection, "output selection ('%s')", rawInstallable());

        auto evaluand = rustEvaluandOf(*this, state, src, prefix);

        auto render = json ? RustRender::Json : raw ? RustRender::Raw : RustRender::ValuePrinter;
        auto text = rustEvalRender(*state, evaluand, render, render == RustRender::ValuePrinter);

        if (raw) {
            logger->stop();
            writeFull(getStandardOutput(), text);
        } else if (json) {
            // Re-dumped rather than printed: `--json` is pretty by default on
            // a terminal, and letting the Rust side format it would give two
            // spellings of the same document that could drift. Parsing here
            // means one formatter, cppnix's, for both backends.
            printJSON(nlohmann::json::parse(text));
        } else {
            logger->cout("%s", text);
        }
    }

    void run(ref<Store> store, ref<InstallableValue> installable) override
    {
        if (raw && json)
            throw UsageError("--raw and --json are mutually exclusive");

        auto state = getEvalState();
        // Here and not earlier: see `shadowState`.
        shadowState = state.get_ptr();

        auto shadowing = evalSettings.evalBackend.get() == "shadow";

        /* `toValue` is what locks a flake, so the installable can only be
           described afterwards -- and it has to be described even when it
           throws, or every flake whose attribute is missing or whose
           evaluation fails would be counted `cpp-failed-before-eval` and
           never compared. Two arms failing the same way is exactly as much of
           a comparison as two arms agreeing, and on somebody iterating on a
           configuration the failures are most of the interesting cases.

           The lock survives the throw: `getCursors` opens the eval cache,
           which needs the locked flake, before it walks any attribute path.
           The rethrow is unconditional, so the served arm's behaviour is
           exactly what it was. */
        auto describe = [&]() {
            if (shadowing)
                describeShadowInstallable(*state, *installable);
        };

        Value * v;
        PosIdx pos;
        try {
            std::tie(v, pos) = installable->toValue(*state);
        } catch (Error & e) {
            describe();
            if (shadowSubject && !shadowAccounted) {
                shadowAccounted = true;
                rustEvalShadow(*state, *shadowSubject, ShadowCppOutcome{false, "", shadowErrorClass(e), e.message()});
            }
            throw;
        }
        describe();

        NixStringContext context;

        if (apply) {
            auto vApply = state->allocValue();
            state->eval(state->parseExprFromString(*apply, state->rootPath(".")), *vApply);
            auto vRes = state->allocValue();
            state->callFunction(*vApply, *v, *vRes, noPos);
            v = vRes;
        }

        if (writeTo) {
            logger->stop();

            if (pathExists(*writeTo))
                throw Error("path '%s' already exists", writeTo->string());

            [&](this const auto & recurse, Value & v, const PosIdx pos, const std::filesystem::path & path) -> void {
                state->forceValue(v, pos);
                if (v.type() == nString) {
                    copyContext(v, context);
                    writeFile(path, v.string_view());
                } else if (v.type() == nAttrs) {
                    [[maybe_unused]] bool directoryCreated = std::filesystem::create_directory(path);
                    // Directory should not already exist
                    assert(directoryCreated);
                    for (auto & attr : *v.attrs()) {
                        std::string_view name = state->symbols[attr.name];
                        try {
                            if (name == "." || name == "..")
                                throw Error("invalid file name '%s'", name);
                            recurse(*attr.value, attr.pos, path / name);
                        } catch (Error & e) {
                            e.addTrace(
                                state->positions[attr.pos], HintFmt("while evaluating the attribute '%s'", name));
                            throw;
                        }
                    }
                } else
                    state->error<TypeError>("value at '%s' is not a string or an attribute set", state->positions[pos])
                        .debugThrow();
            }(*v, pos, *writeTo);
        }

        else if (raw) {
            logger->stop();
            auto string = state->coerceToString(noPos, *v, context, "while generating the eval command output");
            shadowAgainst(*state, std::string(*string));
            writeFull(getStandardOutput(), *string);
        }

        else if (json) {
            auto document = printValueAsJSON(*state, true, *v, pos, context, false);
            // The compact dump, which is the spelling the Rust arm returns.
            // `printJSON` still does the printing, so `--pretty` and the
            // terminal check keep their one implementation.
            shadowAgainst(*state, document.dump());
            printJSON(document);
        }

        else {
            ValuePrinter printer(*state, *v, PrintOptions{.force = true, .derivationPaths = true}, &context);
            // Rendered into a string so the comparison sees the same bytes
            // the user does. No behavioural change: `logger->cout` formats
            // the whole line before writing it either way.
            auto text = fmt("%s", printer);
            shadowAgainst(*state, text);
            logger->cout("%s", text);
        }

        state->ensureLazyPathsCopied(context);
    }
};

static auto rCmdEval = registerCommand<CmdEval>("eval");
