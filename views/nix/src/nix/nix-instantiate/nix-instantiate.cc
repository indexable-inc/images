#include "nix/store/globals.hh"
#include "nix/expr/print-ambiguous.hh"
#include "nix/main/shared.hh"
#include "nix/expr/eval.hh"
#include "nix/expr/eval-inline.hh"
#include "nix/expr/get-drvs.hh"
#include "nix/expr/attr-path.hh"
#include "nix/util/signals.hh"
#include "nix/expr/value-to-xml.hh"
#include "nix/expr/value-to-json.hh"
#include "nix/store/store-open.hh"
#include "nix/store/local-fs-store.hh"
#include "nix/cmd/common-eval-args.hh"
#include "nix/cmd/legacy.hh"
#include "man-pages.hh"
#include "rust-eval.hh"
#include "nix/expr/rust-eval-refusal.hh"
#include "nix/expr/shadow-census.hh"
#include "../rust-eval-session.hh"

#include <map>
#include <iostream>
#include <sstream>
#include <exception>
#include <optional>

using namespace nix;

std::filesystem::path gcRoot;
static int rootNr = 0;

enum OutputKind { okPlain, okRaw, okXML, okJSON };

/**
 * What `eval-backend = shadow` should hand the Rust arm for this file, or
 * nothing when shadow is off.
 *
 * Only the source and its directory: the attribute path varies per iteration
 * of the loop below, and the render mode is derived from `output` where that
 * is known, so those are filled in at the comparison rather than here.
 */
struct ShadowSource
{
    std::string source;
    std::string baseDir;
    std::string file;
};

void processExpr(
    EvalState & state,
    const Strings & attrPaths,
    bool parseOnly,
    bool strict,
    Bindings & autoArgs,
    bool evalOnly,
    OutputKind output,
    bool location,
    Expr * e,
    const std::optional<ShadowSource> & shadow = std::nullopt)
{
    if (parseOnly) {
        e->show(state.symbols, std::cout);
        std::cout << "\n";
        return;
    }

    Value vRoot;
    state.eval(e, vRoot);

    for (auto & i : attrPaths) {
        /* Renders into a string when shadow is on so the comparison sees the
           bytes the user sees, and straight to stdout when it is off. Not
           always through the buffer: `nix-instantiate --eval` of a large
           attribute set streams today, and making every invocation hold its
           whole output in memory to serve a mode nobody has enabled would be
           a real cost for no reader. */
        std::ostringstream buffered;
        std::ostream & out = shadow ? static_cast<std::ostream &>(buffered) : std::cout;
        std::exception_ptr failed;
        std::string failureClass, failureMessage;

        try {
            Value & v(*findAlongAttrPath(state, i, autoArgs, vRoot).first);
            state.forceValue(v, v.determinePos(noPos));

            NixStringContext context;
            if (evalOnly) {
                Value vRes;
                if (autoArgs.empty())
                    vRes = v;
                else
                    state.autoCallFunction(autoArgs, v, vRes);
                if (output == okRaw)
                    out << *state.coerceToString(noPos, vRes, context, "while generating the nix-instantiate output");
                // We intentionally don't output a newline here. The default PS1 for Bash in NixOS starts with a newline
                // and other interactive shells like Zsh are smart enough to print a missing newline before the prompt.
                else if (output == okXML)
                    printValueAsXML(state, strict, location, vRes, out, context, noPos);
                else if (output == okJSON) {
                    printValueAsJSON(state, strict, vRes, v.determinePos(noPos), out, context);
                    out << std::endl;
                } else {
                    if (strict)
                        state.forceValueDeep(vRes);
                    std::set<const void *> seen;
                    printAmbiguous(state, vRes, out, &seen, &context);
                    out << std::endl;
                }
            } else {
                PackageInfos drvs;
                getDerivations(state, v, "", autoArgs, drvs, false);
                for (auto & i : drvs) {
                    auto drvPath = i.requireDrvPath();
                    auto drvPathS = state.store->printStorePath(drvPath);

                    /* What output do we want? */
                    std::string outputName = i.queryOutputName();
                    if (outputName == "")
                        throw Error("derivation '%1%' lacks an 'outputName' attribute", drvPathS);

                    if (gcRoot.empty())
                        printGCWarning();
                    else {
                        auto rootName = absPath(gcRoot);
                        if (++rootNr > 1)
                            rootName += "-" + std::to_string(rootNr);
                        auto store2 = state.store.dynamic_pointer_cast<LocalFSStore>();
                        if (store2)
                            drvPathS = store2->addPermRoot(drvPath, rootName).string();
                    }
                    out << fmt("%s%s\n", drvPathS, (outputName != "out" ? "!" + outputName : ""));
                }
            }

            state.ensureLazyPathsCopied(context);
        } catch (Error & e) {
            /* Only shadow swallows this, and only long enough to compare and
               rethrow. The two arms failing the same way is as much of a
               comparison as the two agreeing, and on a real workload the
               expressions that do not evaluate are a large share of the
               interesting ones. */
            if (!shadow)
                throw;
            failed = std::current_exception();
            failureClass = shadowErrorClass(e);
            failureMessage = e.message();
        }

        if (shadow) {
            /* The user's output first, so a slow or noisy shadow arm cannot
               reorder itself in front of the answer. */
            std::cout << buffered.str();

            auto subject = shadowSubjectOfSource(
                RustSource{shadow->source, shadow->baseDir, shadow->file},
                i,
                output == okJSON  ? RustRender::Json
                : output == okRaw ? RustRender::Raw
                                  : RustRender::Plain);

            /* The trailing newline belongs to this function, not to the
               value. The Rust arm renders the value alone, so leaving it
               attached would report every single evaluation as a divergence
               -- a comparison harness whose every row is its own formatting
               is worse than none. */
            auto text = buffered.str();
            while (!text.empty() && text.back() == '\n')
                text.pop_back();

            rustEvalShadow(
                state,
                subject,
                failed ? ShadowCppOutcome{false, "", failureClass, failureMessage}
                       : ShadowCppOutcome{true, text, "", ""});

            if (failed)
                std::rethrow_exception(failed);
        }
    }
}

static int main_nix_instantiate(int argc, char ** argv)
{
    {
        Strings files;
        bool readStdin = false;
        bool fromArgs = false;
        bool findFile = false;
        bool evalOnly = false;
        bool parseOnly = false;
        OutputKind outputKind = okPlain;
        bool xmlOutputSourceLocation = true;
        bool strict = false;
        Strings attrPaths;
        bool wantsReadWrite = false;

        struct MyArgs : LegacyArgs, MixEvalArgs
        {
            using LegacyArgs::LegacyArgs;
        };

        MyArgs myArgs(std::string(baseNameOf(argv[0])), [&](Strings::iterator & arg, const Strings::iterator & end) {
            if (*arg == "--help")
                showManPage("nix-instantiate");
            else if (*arg == "--version")
                printVersion("nix-instantiate");
            else if (*arg == "-")
                readStdin = true;
            else if (*arg == "--expr" || *arg == "-E")
                fromArgs = true;
            else if (*arg == "--eval" || *arg == "--eval-only")
                evalOnly = true;
            else if (*arg == "--read-write-mode")
                wantsReadWrite = true;
            else if (*arg == "--parse" || *arg == "--parse-only")
                parseOnly = evalOnly = true;
            else if (*arg == "--find-file")
                findFile = true;
            else if (*arg == "--attr" || *arg == "-A")
                attrPaths.push_back(getArg(*arg, arg, end));
            else if (*arg == "--add-root")
                gcRoot = getArg(*arg, arg, end);
            else if (*arg == "--indirect")
                ;
            else if (*arg == "--raw")
                outputKind = okRaw;
            else if (*arg == "--xml")
                outputKind = okXML;
            else if (*arg == "--json")
                outputKind = okJSON;
            else if (*arg == "--no-location")
                xmlOutputSourceLocation = false;
            else if (*arg == "--strict")
                strict = true;
            else if (*arg == "--dry-run")
                settings.readOnlyMode = true;
            else if (*arg != "" && arg->at(0) == '-')
                return false;
            else
                files.push_back(*arg);
            return true;
        });

        myArgs.parseCmdline(argvToStrings(argc, argv));

        if (evalOnly && !wantsReadWrite)
            settings.readOnlyMode = true;

        auto store = openStore();
        auto evalStore = myArgs.evalStoreUrl ? openStore(StoreReference{*myArgs.evalStoreUrl}) : store;

        auto state = std::make_shared<EvalState>(myArgs.lookupPath, evalStore, fetchSettings, evalSettings, store);

        // On a scope guard rather than a straight-line call at the end, so the
        // stats survive a throw.
        //
        // A refusal is fatal: it throws, and every statement after it is
        // skipped. `maybePrintStats()` sat below the work, so the one run that
        // had a refusal to report was exactly the run that reported nothing --
        // the counters read empty precisely when they had something to say.
        // `nix eval` never had this problem because `EvalCommand::~EvalCommand`
        // runs during unwinding; this is nix-instantiate catching up.
        //
        // The journal line is still the production census, for the reason
        // argued on `RefusalCensus`: this only helps a process that gets to
        // unwind, and says nothing about one that is killed.
        struct PrintStatsOnTheWayOut
        {
            EvalState & state;

            ~PrintStatsOnTheWayOut()
            {
                try {
                    state.maybePrintStats();
                } catch (const std::exception & e) {
                    // Reporting must never replace the error being reported,
                    // and throwing from a destructor while unwinding calls
                    // std::terminate. Named on stderr rather than swallowed,
                    // because "the stats did not print" is itself something
                    // the reader needs to know.
                    std::cerr << "nix-instantiate: could not print stats: " << e.what() << std::endl;
                } catch (...) {
                    std::cerr << "nix-instantiate: could not print stats" << std::endl;
                }
            }
        } printStatsOnTheWayOut{*state};

        state->repair = myArgs.repair;

        Bindings & autoArgs = *myArgs.getAutoArgs(*state);

        if (attrPaths.empty())
            attrPaths = {""};

        if (findFile) {
            for (auto & i : files) {
                auto p = state->findFile(i);
                if (auto fn = p.getPhysicalPath())
                    std::cout << fn->string() << std::endl;
                else
                    throw Error("'%s' has no physical path", p);
            }
            return 0;
        }

        auto useRustEval = state->settings.evalBackend == "rust";
        /* Shadow serves the C++ arm, so nothing here refuses and nothing here
           routes: the only difference is that `processExpr` is handed the
           source so it can run the Rust arm afterwards and compare. */
        auto useShadow = state->settings.evalBackend == "shadow";
        if (useRustEval) {
            experimentalFeatureSettings.require(Xp::RustEval);
            if (!evalOnly || parseOnly)
                refuse(refusalTokens::unsupported, "nix-instantiate without --eval");
        }

        if (readStdin) {
            if (useRustEval)
                refuse(refusalTokens::stdinSource, "reading from stdin");
            /* Under shadow, stdin is a skip and not a refusal: the C++ arm
               answers it, and the Rust arm has no path to a source it cannot
               be handed a second time. */
            if (useShadow)
                ShadowCensus::skipped(ShadowSkip::UnservableShape);
            Expr * e = state->parseStdin();
            processExpr(
                *state, attrPaths, parseOnly, strict, autoArgs, evalOnly, outputKind, xmlOutputSourceLocation, e);
        } else if (files.empty() && !fromArgs)
            files.push_back("./default.nix");

        for (auto & i : files) {
            if (useRustEval) {
                if (fromArgs)
                    // `--expr` has no file behind it, which is what cppnix's
                    // own `__curPos` sees: a string origin, answered as null.
                    rustEvalPrint(*state, i, absPath("."), "", attrPaths, outputKind, xmlOutputSourceLocation, strict);
                else {
                    auto path = resolveExprPath(lookupFileArg(*state, i)).path.abs();
                    rustEvalPrint(
                        *state,
                        readFile(path),
                        std::filesystem::path(path).parent_path().string(),
                        path,
                        attrPaths,
                        outputKind,
                        xmlOutputSourceLocation,
                        strict);
                }
                continue;
            }
            /* The same reading the Rust arm above does, so shadow compares
               two arms evaluating one program. `--expr` has no file behind
               it, which is what cppnix's own `__curPos` sees: a string
               origin, answered as null. */
            std::optional<ShadowSource> shadow;
            /* Resolved once and reused by both the shadow description and the
               parse. Resolving twice is not free: `lookupFileArg` reaches the
               source accessor, and a second trip through it made cppnix
               answer `__curPos` as though the file were a single line
               (`[ 1 17 1 35 ]` where a plain run says `[ 3 7 4 9 ]`). Shadow
               changing the C++ arm's answer is the one failure mode that
               invalidates every number this mode produces. */
            std::optional<SourcePath> resolved;
            if (!fromArgs)
                resolved = resolveExprPath(lookupFileArg(*state, i));
            if (useShadow) {
                if (fromArgs)
                    shadow = ShadowSource{i, absPath(".").string(), ""};
                else {
                    auto path = resolved->path.abs();
                    shadow = ShadowSource{readFile(path), std::filesystem::path(path).parent_path().string(), path};
                }
            }

            /* Everything from the parse onwards is inside the shadow's reach,
               not just the per-attribute rendering.

               The first corpus run measured 261 cases and 157 attempts, and
               the 104 missing ones were not skips, refusals or anything else
               with a name: they were the cases that die in the parser or in
               the root evaluation, before `processExpr`'s loop is entered. A
               census with a hundred silently unmeasured cases reports its
               zeros about the 157 it happened to see, which is exactly the
               reading "no divergences" must not be allowed to have. */
            auto attemptsBefore = ShadowCensus::attempts();
            try {
                Expr * e = fromArgs ? state->parseExprFromString(i, state->rootPath("."))
                                    : state->parseExprFromFile(*resolved);
                processExpr(
                    *state,
                    attrPaths,
                    parseOnly,
                    strict,
                    autoArgs,
                    evalOnly,
                    outputKind,
                    xmlOutputSourceLocation,
                    e,
                    shadow);
            } catch (Error & e) {
                /* Only if nothing was compared for this file. A failure after
                   some attribute already agreed is that attribute's business
                   and has been counted; counting it again here would credit
                   one evaluation with two verdicts. */
                if (shadow && ShadowCensus::attempts() == attemptsBefore) {
                    auto subject = shadowSubjectOfSource(
                        RustSource{shadow->source, shadow->baseDir, shadow->file},
                        attrPaths.empty() ? std::string() : attrPaths.front(),
                        outputKind == okJSON  ? RustRender::Json
                        : outputKind == okRaw ? RustRender::Raw
                                              : RustRender::Plain);
                    rustEvalShadow(*state, subject, ShadowCppOutcome{false, "", shadowErrorClass(e), e.message()});
                }
                throw;
            }
        }

        return 0;
    }
}

static RegisterLegacyCommand r_nix_instantiate("nix-instantiate", main_nix_instantiate);
