#pragma once
///@file The C++ side of nix-eval-rs's handle API (rust/nix-eval-rs/src/capi.rs).
///
/// One seam, used by every command that evaluates with `eval-backend = rust`.
/// `rust-eval.cc` keeps the one-call string path for nix-instantiate's
/// whole-expression case, and takes its error translation from here so there
/// is one mapping from a Rust status to a cppnix exception rather than two.

#include "nix/expr/eval.hh"
#include "nix/cmd/command.hh"
#include "nix/store/path.hh"
#include "nix/store/derived-path.hh"
#include "nix/cmd/installable-value.hh"

#include <optional>

#include <exception>
#include <memory>
#include <string>

/// The Rust evaluator's host vtable (`rust/nix-eval-rs/include/ixe.h`) and
/// this file's context object for it.
///
/// Named here rather than included, so a command that only wants
/// `rustEvalSelect` does not pull the C ABI in, and declared at global scope
/// because that is where the C ABI declares the first of them. `RustEvalHost`
/// is defined in `rust-eval-session.cc`; nothing outside it needs the
/// contents.
struct IxeHostVtable;

namespace nix {

struct RustEvalHost;

/// Which bytes a command wants out of the value it selected. Mirrors
/// `IXE_RENDER_*`; rendering happens on the Rust side, where all three
/// walkers already exist and are diffed against cppnix by the lang corpus.
enum class RustRender {
    /// `nix-instantiate --eval --strict`: cppnix's `printAmbiguous`.
    Plain,
    /// `nix eval` with no output flag: cppnix's `ValuePrinter`, which is a
    /// different function from `printAmbiguous` and does not always agree
    /// with it. The Rust side refuses the cases they disagree about rather
    /// than answering in the other dialect.
    ValuePrinter,
    /// `nix eval --json`. Comes back compact; the caller re-dumps it through
    /// nlohmann so `--pretty` cannot drift between the backends.
    Json,
    /// `nix eval --raw`.
    Raw,
    /// `nix-instantiate --eval --strict --xml --no-location`:
    /// builtins.toXML's walker, which is the same printValueAsXML cppnix
    /// calls for both once --no-location turns source positions off. The
    /// document already ends in a newline, so the caller prints it without
    /// appending one.
    Xml,
};

/// Everything the Rust evaluator has to be told before one evaluation, and
/// the teardown of the parts that are per-call.
///
/// One object rather than a block copied into each caller: the block had
/// already drifted, and the copy in the handle path was missing the
/// store-copy hook, so `"${./f}"` reported itself unimplemented through
/// `nix eval` while `nix-instantiate` copied the file. Construct one at the
/// top of any function that is about to call into nix-eval-rs.
struct RustEvalSetup
{
    explicit RustEvalSetup(EvalState & state);
    ~RustEvalSetup();

    RustEvalSetup(const RustEvalSetup &) = delete;
    RustEvalSetup & operator=(const RustEvalSetup &) = delete;

    /// Who answers this evaluation's questions about the outside world: pass
    /// it to `ixe_session_new` or `ixe_eval_expr`.
    ///
    /// Valid for this object's lifetime and no longer. The evaluator copies
    /// the struct, but everything it points at -- the context object, and the
    /// buffers each answer is written into -- belongs to this `RustEvalSetup`,
    /// so a session must not outlive the setup that built its host.
    ///
    /// A pointer rather than a set of `ixe_set_*` calls because the host is
    /// per session. It used to be process state, so two sessions in one
    /// process shared one host and the second to be created silently
    /// answered out of the first's `EvalState`.
    const IxeHostVtable * host() const;

private:
    std::unique_ptr<RustEvalHost> hostState;
};

/// Raise the cppnix exception a nix-eval-rs status stands for.
///
/// Never returns: status 0 is the caller's business and everything else is a
/// failure. Kept in one place because the mapping is a contract -- a thrown
/// error has to arrive as `ThrownError` under the trace note cppnix uses, or
/// the corpus differ reads the class from the wrong exception.
/**
 * Raise the exception a Rust status means, recording the refusal if it is one.
 *
 * `token` names the refusal for the census when `status` is
 * IXE_ERR_UNIMPLEMENTED. It defaults to `unrecorded` because one caller
 * genuinely has no token to give: `ixe_eval_expr` runs without a session, so
 * nothing on that path holds one. Defaulting to a real-looking token would
 * put invented rows in the histogram; defaulting to the sentinel makes the
 * gap countable.
 *
 * `pos` is where in the user's source the failure happened, or null when it
 * happened nowhere the evaluator can name. It becomes the error's own
 * position, which is what makes cppnix print `at /path/file.nix:LINE:COL`
 * and the offending line underneath -- the whole of ENG-12137's user-visible
 * half. Null is a real answer and prints the message alone, exactly as
 * cppnix does for an error it cannot place.
 */
[[noreturn]] void rustEvalThrow(
    EvalState & state,
    int status,
    const std::string & message,
    std::string_view token = "unrecorded",
    std::shared_ptr<const Pos> pos = nullptr);

/// Turn the position nix-eval-rs reported into one cppnix can render.
///
/// `line == 0` is the evaluator saying it has no position and yields null.
/// `file` is the path the failing expression was read from, or null when the
/// source was a string with no file behind it -- in which case `source`
/// becomes the origin, so `--expr` errors print `at «string»:L:C` with the
/// expression quoted underneath, the way cppnix prints them.
///
/// Takes the pieces rather than the ABI's own struct so this header does not
/// have to pull in `ixe.h`, which only exists in a build configured with
/// `-Drust-eval=true`.
std::shared_ptr<const Pos>
rustEvalPos(EvalState & state, const std::string & source, const char * file, uint32_t line, uint32_t column);

/// Evaluate `source`, walk `attrPath` through the handle API, and render what
/// is there.
///
/// Selection does not force what it did not select: `hello.meta.description`
/// out of a large set enters `hello`, `meta` and `description`, and nothing
/// else. That is the reason this exists rather than a call that renders the
/// whole expression and picks text out of the result.
///
/// Throws with the marker "rust-eval unimplemented" on anything the backend
/// or this bridge does not cover, naming it.
///
/// `nestedFailureIsUnimplemented` is for `nix eval`'s plain output, which
/// prints `«error: ...»` for a value that fails inside a structure and keeps
/// going, and this printer does not. With it set, a failure raised while
/// rendering -- after the selected value itself forced cleanly, so the
/// failure is necessarily below the root -- is reported as an unimplemented
/// construct rather than as the evaluation error it is, because cppnix would
/// not have failed there at all.
/// `file` is the absolute path `source` was read from, or empty when it was
/// not read from one (`--expr`). It is what `__curPos` reports, and cppnix
/// answers `null` rather than naming a file for the second case, so the two
/// are distinguished rather than defaulted. ENG-12713.
/// The expression a command was pointed at, resolved to the three things
/// nix-eval-rs is handed.
///
/// One function rather than a block in each command, for the reason
/// `RustEvalSetup` is one object: the block was already in `nix eval`, and
/// `nix build` needs exactly the same one, including the refusals. A second
/// copy is a second set of rules about what `--file` accepts, and the two
/// would answer differently the first time either is touched.
struct RustSource
{
    std::string source;
    std::string baseDir;
    /// The absolute path `source` was read from, or empty when it was not
    /// read from one (`--expr`). What `__curPos` reports; the two are
    /// distinguished rather than defaulted, because cppnix answers `null` for
    /// a string origin. ENG-12713.
    std::string file;
};

/// Read `--expr` or `--file`, or nothing when the `--file` argument is not a
/// plain path.
///
/// The reading rule and nothing else. What an undescribable source *means* is
/// the caller's: `nix eval`'s shadow arm treats it as a skip nobody needs to
/// hear about, and a served command treats it as a refusal the user must be
/// told about. One reader under both, because two would let the arms evaluate
/// different programs while reporting on each other.
std::optional<RustSource> rustReadSource(SourceExprCommand & cmd);

/// Read `--expr` or `--file` for a command the Rust backend is serving, or
/// nothing when the command was given neither and its positional arguments
/// are therefore installables.
///
/// Refuses, by name, every source shape this backend does not read: an
/// expression on stdin and a `--file` that is a flake ref or a lookup path.
/// Also relaxes `pure-eval` for `--file` exactly as `parseInstallables` does,
/// so the two backends agree about what a file argument means -- which is why
/// this must run before any caller builds an `EvalState`, whose constructor
/// captures the restricted accessor.
///
/// `nullopt` is not a refusal. It used to be: with no `--expr` and no
/// `--file` this raised `command-installable`, which is where every flake
/// invocation stopped. Resolving what the positional argument means needs an
/// evaluator, so it moved to `rustEvaluandOf`, one phase later.
std::optional<RustSource> rustSourceOf(SourceExprCommand & cmd);

/// Refuse `--arg`/`--argstr`, which bind free variables before evaluation and
/// which this backend does not carry.
///
/// Only for an `--expr`/`--file` source. A flake installable raises cppnix's
/// own `'--arg' and '--argstr' are incompatible with flakes` from the
/// `InstallableFlake` constructor, and refusing first would report a gap in
/// this backend where cppnix has a usage error.
void rustRequireNoAutoArgs(SourceExprCommand & cmd, EvalState & state);

/// One value the bridge builds and hands to the evaluator.
///
/// Two kinds because `call-flake.nix` takes two kinds: data cppnix computed
/// (the lock file, the overrides set) and one of cppnix's internal primops
/// (`fetchFinalTree`). Both cross through the general handle calls
/// `ixe_alloc_json` and `ixe_internal_primop`; neither the ABI nor the
/// evaluator knows a flake is being built.
struct RustArgument
{
    enum class Kind {
        /// A JSON document in `ixe_alloc_json`'s dialect, which is
        /// `builtins.fromJSON`'s plus a `{"__storePath": "..."}` escape for a
        /// string that must carry a store path as its context.
        Json,
        /// The registered name of one of cppnix's `.internal = true` primops.
        InternalPrimop,
    };

    Kind kind;
    /// The document, or the primop's name.
    std::string text;
};

/// Everything one installable resolves to for the Rust backend: what to
/// evaluate, what to apply it to, and where to look in the result.
///
/// One type for all three source kinds, because after this point they are the
/// same job. `--expr` and `--file` carry no arguments and exactly one
/// attribute path; a flake carries `call-flake.nix`'s three arguments and the
/// candidate list `InstallableFlake::getActualAttrPaths` produced.
struct RustEvaluand
{
    RustSource src;

    /// Applied to `src`'s value in order, before selection. Empty for
    /// `--expr` and `--file`.
    std::vector<RustArgument> args;

    /// Attribute paths to try in order; the first that resolves is the one
    /// selected, which is `InstallableFlake::getCursor` taking `.at(0)` of
    /// the cursors that exist. Exactly one entry for `--expr` and `--file`.
    Strings attrPaths;

    /// Whether an all-digit path component indexes a list.
    ///
    /// True for `--expr`/`--file`, where cppnix walks with
    /// `findAlongAttrPath` (`attr-path.cc`), which does. False for a flake,
    /// where it walks with `AttrCursor::findAlongAttrPath`
    /// (`eval-cache.cc:514`), which only ever calls `maybeGetAttr` -- so
    /// `<flake>#xs.0` is a missing *attribute* named `0` in cppnix, and
    /// indexing here would answer where cppnix reports nothing found.
    bool indexLists = true;

    /// The flake reference, for the "does not provide attribute" message,
    /// and absent for the other two source kinds -- whose single missing
    /// attribute keeps `AttrPathNotFound` and its suggestions.
    std::optional<std::string> flakeRef;
};

/// Resolve one raw installable into what the Rust backend evaluates.
///
/// The second phase of source resolution. `rustSourceOf` runs before there is
/// an `EvalState`, because it moves `pure-eval`; this runs after, because a
/// flake reference has to be locked and locking needs an evaluator, a store
/// and the registry.
///
/// `source` is what `rustSourceOf` returned. When it holds a value the
/// positional argument is an attribute path into it and nothing is fetched.
/// When it is empty the positional argument is an installable, and this
/// refuses a store path by name and locks a flake reference through
/// cppnix's own `lockFlake` -- which stays cpp, as it must: it walks the
/// input graph, hits the registry and writes `flake.lock`, all of which is IO
/// and policy the evaluator does not decide.
///
/// What crosses afterwards is `call-flake.nix` (ordinary Nix, 105 lines) and
/// its three arguments. No `LockedFlake` reaches the VM.
RustEvaluand rustEvaluandOf(
    SourceExprCommand & cmd, ref<EvalState> state, const std::optional<RustSource> & source, std::string_view prefix);

/// One derivation the Rust backend selected, reduced to what a build needs.
///
/// This is deliberately not a `Value`: the Rust VM's values live in its own
/// handle table and never become cppnix `Value`s, so what crosses is the
/// answer rather than the object. `nix build` needs a drvPath and the set of
/// outputs to install, and those are the whole of it.
struct RustDerivation
{
    StorePath drvPath;
    /// The outputs to install, already reduced by `meta.outputsToInstall`.
    /// Never empty: cppnix defaults to `out`.
    StringSet outputs;
};

/// Evaluate `source`, walk `attrPath`, and report the derivation found there.
///
/// The same session and the same handle walk `rustEvalSelect` performs -- one
/// evaluation pipeline, not two -- with a different question asked at the
/// end: instead of rendering the value, this reads the handful of attributes
/// cppnix's `PackageInfo` reads (`get-drvs.cc`), and answers with the
/// derivation they name.
///
/// Refuses, by name, everything in that shape it does not cover: a value that
/// is not a derivation, an `outputs` list this cannot read, and any
/// `outputsToInstall` shape whose reduction is not a plain subset. Never a
/// silent fallback and never a guess: a wrong output set is a wrong build.
std::vector<RustDerivation> rustEvalDerivations(EvalState & state, const RustEvaluand & evaluand);

/// Evaluate `evaluand`, apply its arguments, select, and render.
///
/// The whole served pipeline for a command that prints a value. `nix build`
/// takes the same pipeline as far as selection and then asks
/// `rustEvalDerivations` a different question of the value it reached.
std::string rustEvalRender(
    EvalState & state, const RustEvaluand & evaluand, RustRender render, bool nestedFailureIsUnimplemented = false);

/// The same, for a source and one attribute path with no arguments.
///
/// Kept as its own entry point for the shadow arm, which describes an
/// evaluation the C++ backend already served and has no installable to
/// resolve.
std::string rustEvalSelect(
    EvalState & state,
    const std::string & source,
    const std::string & baseDir,
    const std::string & file,
    const std::string & attrPath,
    RustRender render,
    bool nestedFailureIsUnimplemented = false);

/// What a shadow comparison asks of the value the two arms selected.
///
/// Two questions, because two commands want two different answers out of one
/// evaluation. `nix eval` wants the rendered bytes; `nix build` wants a
/// derivation, which is not printable text at all and which no render mode
/// produces. Comparing a build by rendering its value would compare the
/// printed form of a derivation attrset -- a large, position-dependent
/// document full of things neither arm promises to spell alike -- instead of
/// the drvPath, which is Tier 1 and is the whole point.
enum class ShadowQuestion {
    /// The rendered value, in `ShadowSubject::render`'s dialect.
    Render,
    /// The derivation the value names: its drvPath and the outputs to
    /// install, as `rustEvalDerivations` reports them.
    Derivation,
};

/// What the Rust arm should evaluate so that it is evaluating the same thing
/// the C++ arm just did, and what to ask of it.
///
/// Carries a whole `RustEvaluand` rather than a source and an attribute path.
/// It used to carry the loose pair, and that is precisely why shadow could
/// never see a flake: `--expr` and `--file` are the only evaluations that fit
/// in a source plus one path, so every flake installable was turned away at
/// the description step and counted `unservable-shape` -- which is how
/// `darwin-rebuild` and `home-manager` came to report `attempts: 0` for ever
/// while the `rust` backend served the very same commands. `RustEvaluand` is
/// the type the served path already resolves an installable into, so sharing
/// it means the two arms cannot come to disagree about what a command line
/// meant.
struct ShadowSubject
{
    RustEvaluand evaluand;
    RustRender render = RustRender::Plain;
    bool nestedFailureIsUnimplemented = false;
    ShadowQuestion question = ShadowQuestion::Render;
    /// How the user named this evaluation, when that is more use than the
    /// file and attribute path -- an installable is `nixpkgs#hello`, which no
    /// field of the evaluand spells (the evaluand's source is
    /// `call-flake.nix`). Empty for an `--expr`/`--file` source, whose file
    /// and attribute path are the better answer.
    std::string what;
};

/// A subject for an `--expr`/`--file` source and one attribute path.
///
/// The other half of `shadowEvaluandOfInstallable`: three commands describe a
/// plain source and each used to spell the evaluand out by hand, which is
/// three places for `indexLists` or the base directory to drift into
/// disagreement about what the same command line meant.
ShadowSubject shadowSubjectOfSource(
    const RustSource & src, const std::string & attrPath, RustRender render, bool nestedFailureIsUnimplemented = false);

/// Describe an installable the C++ arm has already resolved, or count a named
/// skip and answer nothing.
///
/// The seam that lets shadow see a flake. It reuses the `InstallableFlake`
/// the served arm built, and therefore its **already computed** `LockedFlake`
/// -- it never calls `lockFlake` itself. That is not an optimisation: locking
/// walks the input graph, consults the registry and writes `flake.lock`, and
/// a comparison harness that re-ran all of that would be doing IO on the
/// user's behalf that the user did not ask for a second time, after the arm
/// that serves has already finished. Call it only once the C++ arm has forced
/// the installable, which is what populates the lock.
///
/// `nullopt` means "counted as skipped, with a reason". It never throws and
/// never refuses: under shadow the C++ arm has answered whatever this cannot
/// describe, so nobody is denied anything, and the census gets a row saying
/// which kind of argument went uncompared.
std::optional<RustEvaluand> shadowEvaluandOfInstallable(EvalState & state, Installable & installable);

/// The C++ arm's derivations in the bytes a `Derivation` question is compared
/// on, or nothing when the shape is not comparable (skip counted).
///
/// One spelling for both arms, defined once here, because two encoders is two
/// answers to "did they agree" and the harness would be reporting on its own
/// formatting.
std::optional<std::string> shadowDerivationText(Store & store, const DerivedPaths & paths);

/// What the C++ arm -- the arm that is served -- produced.
///
/// Either a rendered value or a failure, never both. The failure carries the
/// exception's class as well as its text, because the two are compared
/// against different bars: a different class is a real divergence, while
/// different wording is tier 2, where functional equivalence suffices.
struct ShadowCppOutcome
{
    bool ok = false;
    std::string text;
    std::string errorClass;
    std::string errorMessage;
};

/// The class name for a caught exception, for comparing failures.
///
/// Its own function so the two arms are classified by one rule. Matches on
/// the exception type rather than on the message, which is what makes this
/// independent of `maintainers/ix/error-class.sh` instead of a second, drifting
/// copy of it: that script classifies text because all it has is a log, and
/// this has the object.
std::string shadowErrorClass(const std::exception & e);

/// Evaluate `subject` with the Rust backend, compare against `cpp`, record.
///
/// **Never throws.** Everything the Rust arm can do -- refuse, disagree,
/// fail, crash -- is caught and counted, because the user has already been
/// served the C++ answer and a shadow that could fail a command would be a
/// shadow nobody dares leave on.
///
/// Counts one attempt before entering the Rust arm and exactly one verdict
/// after, so an arm that never comes back shows up as `unaccounted` in the
/// stats block instead of vanishing.
void rustEvalShadow(EvalState & state, const ShadowSubject & subject, const ShadowCppOutcome & cpp);

} // namespace nix
