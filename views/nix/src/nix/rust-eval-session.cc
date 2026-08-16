#include "rust-eval-session.hh"
#include "nix/expr/eval-perf-census.hh"
#include "nix/expr/rust-eval-refusal.hh"
#include "nix/expr/shadow-census.hh"

#include "nix/util/error.hh"
#include "nix/util/users.hh" // getHome, for `~/...` path literals
#include "nix/util/util.hh"
#include "nix/expr/eval-error.hh"
#include "nix/expr/attr-path.hh"
#include "nix/store/globals.hh" // nixVersion
#include "nix/store/store-api.hh"
#include "nix/fetchers/fetch-to-store.hh"
#include "nix/fetchers/tarball.hh"
#include "nix/fetchers/registry.hh"
#include "nix/fetchers/input-cache.hh"
#include "nix/flake/flake.hh"
#include "nix/flake/flakeref.hh"
#include "nix/flake/settings.hh"
#include "nix/flake/flakeref.hh"
#include "nix/flake/lockfile.hh"
#include "nix/cmd/installable-flake.hh"
#include "nix/cmd/installable-derived-path.hh"
#include "nix/store/outputs-spec.hh"
#include "nix/expr/value-to-json.hh"

#include <nlohmann/json.hpp>
#include "nix/util/finally.hh"
#include "nix/util/signals.hh" // isInterrupted
#include "nix/util/hash.hh"    // the divergence id

#include <algorithm> // std::sort, over the derivation lines
#include <iostream>
#include <chrono>
#include <cstring> // strnlen, over the names buffer
#include <filesystem>

// Spelled `defined(...) &&` rather than a bare `#if`: -Werror=undef makes an
// undefined macro a build error, and the whole point of the #else path is to
// compile when -Drust-eval is off and the macro does not exist.
#if defined(HAVE_RUST_EVAL) && HAVE_RUST_EVAL
#  include "ixe.h"
#endif

namespace nix {

std::optional<RustSource> rustReadSource(SourceExprCommand & cmd)
{
    if (cmd.expr)
        return RustSource{*cmd.expr, absPath(cmd.getCommandBaseDir()).string(), ""};
    auto arg = cmd.file->string();
    // `<nixpkgs>` is a search-path lookup (ENG-12443) and a flake ref is a
    // fetch; `lookupFileArg` would quietly resolve either through the C++
    // evaluator, which is how a backend comes to be credited with an answer
    // it did not produce.
    if (arg.starts_with("<") || arg.find(':') != std::string::npos)
        return std::nullopt;
    auto dir = absPath(cmd.getCommandBaseDir());
    auto path = absPath(arg, &dir);
    if (std::filesystem::is_directory(path))
        path = path / "default.nix";
    return RustSource{readFile(path.string()), path.parent_path().string(), path.string()};
}

std::optional<RustSource> rustSourceOf(SourceExprCommand & cmd)
{
    if (cmd.file && cmd.expr)
        throw UsageError("'--file' and '--expr' are exclusive");
    // Neither given: the positional argument is an installable, and what it
    // means -- a store path, a flake reference -- cannot be settled without an
    // evaluator. `rustEvaluandOf` settles it one phase later. This used to
    // refuse here, which is where every flake invocation stopped.
    if (!cmd.file && !cmd.expr)
        return std::nullopt;
    if (cmd.file && *cmd.file == "-")
        refuse(refusalTokens::stdinSource, "reading the expression from stdin");

    // `--file` relaxes pure eval in the C++ path; do the same here, and do it
    // before any caller builds an EvalState, because the constructor captures
    // the restricted accessor and a state built first stays restricted.
    //
    // Only on this path. The shadow arm reads the same source through
    // `rustReadSource` and must not come through here, because there the C++
    // evaluator is the one answering and moving its purity would change the
    // very thing the comparison is measuring.
    if (cmd.file) {
        if (evalSettings.pureEval && evalSettings.pureEval.overridden)
            throw UsageError("'--file' is not compatible with '--pure-eval'");
        evalSettings.pureEval = false;
    }

    auto read = rustReadSource(cmd);
    if (!read)
        refuse(refusalTokens::file, "--file '%s' (only a plain path)", cmd.file->string());
    return *read;
}

ShadowSubject shadowSubjectOfSource(
    const RustSource & src, const std::string & attrPath, RustRender render, bool nestedFailureIsUnimplemented)
{
    ShadowSubject subject;
    /* `indexLists` stays true, which is `findAlongAttrPath`'s rule and the
       one the C++ arm just used for this same source. A flake is the case
       that wants it false, and a flake does not come through here. */
    subject.evaluand = RustEvaluand{.src = src, .args = {}, .attrPaths = {attrPath}};
    subject.render = render;
    subject.nestedFailureIsUnimplemented = nestedFailureIsUnimplemented;
    subject.question = ShadowQuestion::Render;
    return subject;
}

void rustRequireNoAutoArgs(SourceExprCommand & cmd, EvalState & state)
{
    if (cmd.getAutoArgs(state)->size())
        refuse(refusalTokens::args, "--arg/--argstr");
}

#if defined(HAVE_RUST_EVAL) && HAVE_RUST_EVAL

namespace {

/// The shadow evaluation running on this thread, if any.
///
/// Declared up here rather than beside `rustEvalShadow`, which is where it is
/// used, because three of the host hooks below have to read it and they are
/// compiled first. What they need it for is the two properties shadow
/// promises and could not otherwise keep:
///
///  - **The served arm's output does not move.** The Rust arm runs the user's
///    expression a second time, so its `builtins.trace` calls, its warnings
///    and its cache complaints are second copies of lines cppnix has already
///    printed. Under `eval-backend = shadow` the C++ answer is the one served
///    and its bytes must be the bytes `cpp` would have produced, so the
///    shadow arm's copies are dropped. Nothing is lost: they are the same
///    lines, from the same expression, and the arm that produced them is the
///    one nobody is reading.
///  - **A single long evaluation is bounded.** `eval-shadow-budget` used to
///    be checked only on the way in, which bounds a command evaluating many
///    small expressions and does nothing at all for a command evaluating one
///    enormous one -- `nix build .#darwinConfigurations.<host>.system`, which
///    is the whole reason this path exists. The interrupt hook the VM already
///    polls turns the budget into a deadline the attempt cannot run past.
struct ShadowAttempt
{
    /// Whether this thread is inside a shadow evaluation.
    ///
    /// The recursion guard the ladder asks for. It bounds the overhead at one
    /// extra evaluation per user evaluation instead of letting it compound,
    /// and it is not hypothetical: the Rust arm answers its own questions
    /// through hooks that run cppnix code (`rustCopyToStore` calls
    /// `copyPathToStore`), and anything down there that evaluated would
    /// otherwise be shadowed again from inside a shadow.
    bool active = false;

    /// When this attempt must stop, or none when the budget is unlimited.
    std::optional<std::chrono::steady_clock::time_point> deadline;

    /// Whether the deadline is what stopped it.
    ///
    /// Read after the call to tell a budget cutoff from a real disagreement.
    /// Without it, every over-budget attempt would be reported as
    /// `rust-failed-cpp-succeeded` with cppnix's "interrupted by the user"
    /// wording, and the divergence histogram would fill up with findings
    /// about this harness rather than about the evaluator.
    bool tripped = false;
};

thread_local ShadowAttempt shadowAttempt;

/// Whether this attempt's deadline has passed.
///
/// The clock is read on every call, with no sampling of its own, because the
/// VM already strides its interrupt checks by `INTERRUPT_CHECK_STRIDE`
/// (2048 poll iterations, `rust/nix-eval-rs/src/vm.rs`) and so this is not on
/// a hot path. A second stride here multiplied the two and pushed the
/// measured cutoff further past the budget for no gain.
bool shadowDeadlinePassed()
{
    return std::chrono::steady_clock::now() >= *shadowAttempt.deadline;
}

} // namespace

/// Everything a host callback needs: the `EvalState` it answers out of, and
/// the buffers it answers into.
///
/// One of these per `RustEvalSetup`, reached through the `ctx` pointer the
/// vtable carries, so a callback's world is the session that installed it.
/// This used to be a `thread_local EvalState *` and fourteen `thread_local
/// std::string`s, because a hook was a bare C function pointer in a
/// process-global slot with nowhere to put a closure: two sessions in one
/// process shared one set of buffers and one notion of "the current state",
/// and the second one to start silently answered out of the first one's.
///
/// Each buffer is separate rather than one shared scratch string. The Rust
/// side copies an answer before returning, so a buffer only has to outlive
/// its own call -- but two hooks can be in flight over one path (`file_type`
/// and `file_type_resolved` ask different questions about it), and a shared
/// buffer would let one overwrite the other between the write and the copy.
struct RustEvalHost
{
    EvalState & state;

    std::string copyToStoreAnswer;
    std::string storeTextAnswer;
    std::string writeDrvAnswer;
    std::string storeFilteredAnswer;
    std::string fetchAnswer;
    std::string fetchTreeAnswer;
    std::string lockFlakeAnswer;
    std::string parseFlakeRefAnswer;
    std::string flakeRefToStringAnswer;
    std::string readFileAnswer;
    std::string readDirAnswer;
    std::string fileTypeAnswer;
    std::string fileTypeResolvedAnswer;
    std::string ensurePathAnswer;
    std::string realiseAnswer;
    std::string realiseCheckAnswer;
    std::string realiseAllowAnswer;
    std::string findFileAnswer;
    std::string nixPathAnswer;

    /// The struct handed to the evaluator, pointing back at this object.
    IxeHostVtable vtable;

    explicit RustEvalHost(EvalState & state);
};

/// The host a callback's context pointer names.
///
/// Not a recoverable case and therefore not a check that returns an error:
/// `RustEvalSetup` sets `ctx` to the host it owns, `ixe_session_new` refuses
/// a null vtable, and the evaluator hands `ctx` back unchanged. A null here
/// would be the ABI misbehaving. Every callback below used to open with
/// `if (!currentState) return answer("no evaluator state", 1)`, which was a
/// real case when the state was a process-wide slot a caller could forget to
/// set; it cannot be one when the state arrives with the call.
static RustEvalHost & hostOf(void * ctx)
{
    assert(ctx);
    return *static_cast<RustEvalHost *>(ctx);
}

/// `"${./f}"` is the store path cppnix copies the file to, not the source
/// path (eval.cc:2582). The evaluator cannot do this itself: the store is
/// ours, and under read-only mode -- which `nix-instantiate --eval` sets --
/// the answer is the path the copy WOULD produce with no bytes moved.
/// ENG-12447.
static int
rustCopyToStore(void * ctx, const unsigned char * path, size_t pathLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.copyToStoreAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.copyToStoreAnswer.data());
        *outLen = host.copyToStoreAnswer.size();
        return rc;
    };

    // A C++ exception must not unwind through Rust frames, so every failure
    // comes back as a status and a message instead.
    try {
        std::string p(reinterpret_cast<const char *>(path), pathLen);
        NixStringContext context;
        auto sourcePath = host.state.rootPath(CanonPath(p));
        auto storePath = host.state.copyPathToStore(context, sourcePath);
        return answer(host.state.store->printStorePath(storePath), 0);
    } catch (Error & e) {
        // The message alone: the Rust arm carries no source positions
        // (ENG-12137), so a trace block here would be the only difference
        // between the two arms' error text.
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.toFile` (`primops.cc:2789`).
///
/// Here rather than in the evaluator for one reason: the *path* is a pure
/// function of the bytes and the references, but whether the bytes are written
/// is `settings.readOnlyMode`, which is ours. `nix-instantiate --eval` computes
/// without writing; `nix build` writes. The evaluator cannot tell those apart
/// and must not guess, so it asks and this answers -- the same seam
/// `rustEnsurePath` uses for the same reason (ENG-12479). ENG-12607.
static int rustStoreText(
    void * ctx,
    const unsigned char * name,
    size_t nameLen,
    const unsigned char * contents,
    size_t contentsLen,
    const unsigned char * references,
    size_t referencesLen,
    const unsigned char ** out,
    size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.storeTextAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.storeTextAnswer.data());
        *outLen = host.storeTextAnswer.size();
        return rc;
    };

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        std::string n(reinterpret_cast<const char *>(name), nameLen);
        std::string c(reinterpret_cast<const char *>(contents), contentsLen);

        // NUL-separated, which is unambiguous because a store path cannot
        // contain one. A trailing partial field would be a malformed call.
        StorePathSet refs;
        std::string_view rest(reinterpret_cast<const char *>(references), referencesLen);
        while (!rest.empty()) {
            auto end = rest.find('\0');
            if (end == std::string_view::npos)
                return answer("rust-eval: unterminated reference in builtins.toFile", 1);
            if (end > 0)
                refs.insert(host.state.store->parseStorePath(rest.substr(0, end)));
            rest.remove_prefix(end + 1);
        }

        auto storePath = settings.readOnlyMode ? host.state.store->makeFixedOutputPathFromCA(
                                                     n,
                                                     TextInfo{
                                                         .hash = hashString(HashAlgorithm::SHA256, c),
                                                         .references = refs,
                                                     })
                                               : ({
                                                     StringSource s{c};
                                                     host.state.store->addToStoreFromDump(
                                                         s,
                                                         n,
                                                         FileSerialisationMethod::Flat,
                                                         ContentAddressMethod::Raw::Text,
                                                         HashAlgorithm::SHA256,
                                                         refs,
                                                         host.state.repair);
                                                 });
        /* cppnix's `prim_toFile` ends in `allowAndSetStorePathString`
           (`primops.cc:2836`): under pure eval the result of
           `builtins.toFile` is readable. Without this line the store path
           comes back unregistered and the Rust arm alone refuses
           `import "${builtins.toFile ...}"` with the AllowList denial.
           `rustWriteDerivation` below shares this body and deliberately
           does NOT allow: cppnix's `writeDerivation` never does. ENG-13138. */
        host.state.allowPath(storePath);
        return answer(host.state.store->printStorePath(storePath), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `writeDerivation` (`derivations.cc:170`), which is where cppnix's
/// `derivationStrictInternal` puts the `.drv` it just built.
///
/// This is the whole reason `nix build` can be served: `nix eval` only ever
/// needs the drvPath, which the evaluator computes for itself, whereas a
/// build needs every `.drv` in the input closure to be a real store object
/// that the daemon can read back. The evaluator is the only thing that sees
/// all of them, so the write has to leave from inside it.
///
/// Deliberately the same body as `rustStoreText` below the branch: cppnix's
/// `writeDerivation` *is* `addTextToStore` of the ATerm, so a second spelling
/// of the hashing or of the read-only rule here would be a mirror to drift.
/// The evaluator compares the answer with the path it computed from the same
/// bytes, so a disagreement between the two implementations of that rule
/// surfaces at the derivation that caused it. ENG-12799.
static int rustWriteDerivation(
    void * ctx,
    const unsigned char * name,
    size_t nameLen,
    const unsigned char * aterm,
    size_t atermLen,
    const unsigned char * references,
    size_t referencesLen,
    const unsigned char ** out,
    size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.writeDrvAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.writeDrvAnswer.data());
        *outLen = host.writeDrvAnswer.size();
        return rc;
    };

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        // `writeDerivation` appends the suffix; the evaluator hands the bare
        // name over so this call reads like cppnix's.
        std::string suffix = std::string(reinterpret_cast<const char *>(name), nameLen) + drvExtension;
        std::string contents(reinterpret_cast<const char *>(aterm), atermLen);

        // NUL-separated, as for `rustStoreText`. Unlike that one's, these
        // references legitimately include `.drv` paths: a derivation refers
        // to the derivations it takes inputs from.
        StorePathSet refs;
        std::string_view rest(reinterpret_cast<const char *>(references), referencesLen);
        while (!rest.empty()) {
            auto end = rest.find('\0');
            if (end == std::string_view::npos)
                return answer("rust-eval: unterminated reference in a derivation write", 1);
            if (end > 0)
                refs.insert(host.state.store->parseStorePath(rest.substr(0, end)));
            rest.remove_prefix(end + 1);
        }

        auto storePath = settings.readOnlyMode ? host.state.store->makeFixedOutputPathFromCA(
                                                     suffix,
                                                     TextInfo{
                                                         .hash = hashString(HashAlgorithm::SHA256, contents),
                                                         .references = refs,
                                                     })
                                               : ({
                                                     StringSource s{contents};
                                                     host.state.store->addToStoreFromDump(
                                                         s,
                                                         suffix,
                                                         FileSerialisationMethod::Flat,
                                                         ContentAddressMethod::Raw::Text,
                                                         HashAlgorithm::SHA256,
                                                         refs,
                                                         host.state.repair);
                                                 });
        return answer(host.state.store->printStorePath(storePath), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.path` (`primops.cc:3073`), whose copy this performs.
///
/// The evaluator has already walked the tree and run the filter -- the filter
/// is a Nix function and only the interpreter can call it -- so what arrives
/// is a set of accepted paths. This turns that set into a `PathFilter` and
/// then does exactly what `addPath` does, rather than reimplementing the
/// archive: same `fetchToStore`, same read-only branch, same expected-hash
/// short circuit. Re-deciding anything here would be a second filter
/// implementation for the two to disagree over. ENG-12678.
static int rustStoreFiltered(
    void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.storeFilteredAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.storeFilteredAnswer.data());
        *outLen = host.storeFilteredAnswer.size();
        return rc;
    };

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        // NUL-terminated fields, the encoding documented beside
        // `ixe_store_filtered_fn` in ixe.h. A missing terminator is a
        // malformed call and not a field with the rest of the buffer in it.
        std::vector<std::string_view> fields;
        std::string_view rest(reinterpret_cast<const char *>(request), requestLen);
        while (!rest.empty()) {
            auto end = rest.find('\0');
            if (end == std::string_view::npos)
                return answer("rust-eval: unterminated field in builtins.path request", 1);
            fields.emplace_back(rest.substr(0, end));
            rest.remove_prefix(end + 1);
        }
        if (fields.size() < 6)
            return answer("rust-eval: builtins.path request is too short", 1);

        std::string root(fields[0]);
        std::string name(fields[1]);

        ContentAddressMethod method = ContentAddressMethod::Raw::NixArchive;
        if (fields[2] == "flat")
            method = ContentAddressMethod::Raw::Flat;
        else if (fields[2] != "nar")
            return answer("rust-eval: unknown ingestion method in builtins.path request", 1);

        std::optional<Hash> expectedHash;
        if (!fields[3].empty())
            expectedHash = Hash::parseAny(fields[3], HashAlgorithm::SHA256);

        // `addPath`'s store-path branch (`primops.cc:2947`). The evaluator
        // decides *whether* it applies -- the root coerced with a context and
        // is under the store directory -- because only it saw the value, and
        // it has already realised that context and rewritten the root. What
        // is left is the store query, which is only ours to make.
        bool inheritReferences = false;
        if (fields[4] == "inherit-references")
            inheritReferences = true;
        else if (fields[4] != "own-references")
            return answer("rust-eval: unknown reference marker in builtins.path request", 1);

        // Membership, not a predicate the evaluator described: the accepted
        // set is closed downwards, so a directory that is absent prunes its
        // whole subtree and `dumpPath` never asks about anything below it.
        std::optional<StringSet> accepted;
        if (fields[5] == "filtered") {
            if ((fields.size() - 6) % 2 != 0)
                return answer("rust-eval: builtins.path request has a half entry", 1);
            StringSet paths;
            for (size_t i = 6; i < fields.size(); i += 2)
                paths.insert(std::string(fields[i]));
            accepted = std::move(paths);
        } else if (fields[5] != "unfiltered")
            return answer("rust-eval: unknown filter marker in builtins.path request", 1);

        std::unique_ptr<PathFilter> filter;
        if (accepted)
            filter = std::make_unique<PathFilter>([&](const std::string & p) { return accepted->count(p) > 0; });

        auto & store = *host.state.store;
        auto sourcePath = host.state.rootPath(CanonPath(root));

        // `addPath` swallows the failure here rather than letting an invalid
        // path abort the copy, and leaves `refs` empty. Transcribed, comment
        // and all, so the two arms agree about a root whose store object went
        // away between the realise and this call.
        StorePathSet refs;
        if (inheritReferences) {
            auto [storePath, subPath] = store.toStorePath(root);
            try {
                refs = store.queryPathInfo(storePath)->references;
            } catch (Error &) { // FIXME: should be InvalidPathError
            }
        }

        std::optional<StorePath> expectedStorePath;
        if (expectedHash)
            expectedStorePath = store.makeFixedOutputPathFromCA(
                name, ContentAddressWithReferences::fromParts(method, *expectedHash, {refs}));

        if (!expectedHash || !store.isValidPath(*expectedStorePath)) {
            // `fetchToStore` cannot carry references (cppnix's own FIXME), so
            // the refs case goes through `addToStore`, which takes a
            // `PathFilter &` rather than a pointer and therefore needs
            // `defaultPathFilter` spelled out where the other arm passes null.
            auto dstPath = refs.empty() ? fetchToStore(
                                              host.state.fetchSettings,
                                              store,
                                              sourcePath.resolveSymlinks(),
                                              settings.readOnlyMode ? FetchMode::DryRun : FetchMode::Copy,
                                              name,
                                              method,
                                              filter.get(),
                                              host.state.repair)
                                        : store.addToStore(
                                              name,
                                              sourcePath.resolveSymlinks(),
                                              method,
                                              HashAlgorithm::SHA256,
                                              refs,
                                              filter ? *filter.get() : defaultPathFilter,
                                              host.state.repair);
            if (expectedHash && expectedStorePath != dstPath)
                return answer(fmt("store path mismatch in (possibly filtered) path added from '%s'", root), 1);
            /* Both exits allow the path, as cppnix's `addPath` does on both
               of its (`primops.cc:2995` and `:2997`): under pure eval the
               result of `builtins.path`/`builtins.filterSource` is readable.
               This omission was 96 of the 100 rust-arm failures in the
               whole-ix sweep: every `lib.cleanSource`d tree came back
               unregistered, and the first read inside it got the AllowList
               denial. ENG-13138. */
            host.state.allowPath(dstPath);
            return answer(store.printStorePath(dstPath), 0);
        }
        host.state.allowPath(*expectedStorePath);
        return answer(store.printStorePath(*expectedStorePath), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.fetchurl` and `builtins.fetchTarball`, from `checkURI` onward.
///
/// The evaluator has already done everything cppnix's `fetch()`
/// (`primops/fetchTree.cc:462`) does before it touches the world: read the
/// argument set, rewrite a `channel:` URL, default the name and validate it,
/// parse the `sha256`. What is left is the IO, and this is a transcription of
/// cppnix's own -- same `ensurePath` early exit, same `downloadFile` and
/// `downloadTarball`, same mismatch check and same message. Nothing here
/// re-derives a name or a URL; doing so would be a second implementation of
/// rules the evaluator already applied, for the two to disagree over.
static int
rustFetch(void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.fetchAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.fetchAnswer.data());
        *outLen = host.fetchAnswer.size();
        return rc;
    };

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        // NUL-terminated fields, the encoding documented beside
        // `ixe_fetch_fn` in ixe.h.
        std::vector<std::string_view> fields;
        std::string_view rest(reinterpret_cast<const char *>(request), requestLen);
        while (!rest.empty()) {
            auto end = rest.find('\0');
            if (end == std::string_view::npos)
                return answer("rust-eval: unterminated field in fetch request", 1);
            fields.emplace_back(rest.substr(0, end));
            rest.remove_prefix(end + 1);
        }
        if (fields.size() != 4)
            return answer("rust-eval: fetch request must have four fields", 1);

        std::string url(fields[0]);
        std::string name(fields[1]);

        bool unpack = false;
        if (fields[2] == "tarball")
            unpack = true;
        else if (fields[2] != "file")
            return answer("rust-eval: unknown kind in fetch request", 1);

        std::optional<Hash> expectedHash;
        if (!fields[3].empty())
            expectedHash = Hash::parseAny(fields[3], HashAlgorithm::SHA256);

        // Reached only when the evaluator asked, and it refuses the whole
        // question channel under restrict-eval, so in this build this is
        // belt and braces. Kept because it is cppnix's check and this is
        // cppnix's code path: the day the evaluator learns to distinguish
        // the two purity settings, the check has to already be here.
        host.state.checkURI(url);

        auto & store = *host.state.store;

        // Early exit if pinned and already in the store. THE hermetic branch:
        // with a sha256 the store path is known before anything is
        // downloaded, and if the store can produce it nothing is.
        if (expectedHash && expectedHash->algo == HashAlgorithm::SHA256) {
            auto expectedPath = store.makeFixedOutputPath(
                name,
                FixedOutputInfo{
                    .method = unpack ? FileIngestionMethod::NixArchive : FileIngestionMethod::Flat,
                    .hash = *expectedHash,
                    .references = {}});
            try {
                store.ensurePath(expectedPath);
                host.state.allowPath(expectedPath);
                return answer(store.printStorePath(expectedPath), 0);
            } catch (Error & e) {
                debug(
                    "substitution of '%s' failed, will try to download: %s",
                    store.printStorePath(expectedPath),
                    e.what());
                // Fall through to download.
            }
        }

        auto storePath = unpack ? fetchToStore(
                                      host.state.fetchSettings,
                                      store,
                                      fetchers::downloadTarball(store, host.state.fetchSettings, url),
                                      FetchMode::Copy,
                                      name)
                                : fetchers::downloadFile(store, host.state.fetchSettings, url, name).storePath;

        if (expectedHash) {
            auto hash = unpack ? store.queryPathInfo(storePath)->narHash
                               : hashPath(
                                     {store.requireStoreObjectAccessor(storePath)},
                                     FileSerialisationMethod::Flat,
                                     HashAlgorithm::SHA256)
                                     .hash;
            if (hash != *expectedHash)
                return answer(
                    fmt("hash mismatch in file downloaded from '%s':\n  specified: %s\n  got:       %s",
                        url,
                        expectedHash->to_string(HashFormat::Nix32, true),
                        hash.to_string(HashFormat::Nix32, true)),
                    1);
        }

        host.state.allowPath(storePath);
        return answer(store.printStorePath(storePath), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// Defined with the rest of the flake machinery, far below; declared here
/// because `rustLockFlake` and `rustEvaluandOf` are the two callers and they
/// sit at opposite ends of this file. One definition on purpose: the overrides
/// document is what decides which tree every flake input resolves to, and two
/// copies of it is how `getFlake` and the command line come to disagree.
static nlohmann::json flakeOverridesJSON(EvalState & state, const flake::LockedFlake & lockedFlake);

/// `builtins.getFlake`, from `parseFlakeRef` up to but not including
/// `callFlake`.
///
/// This is the first half of cppnix's `prim_getFlake`
/// (`libflake/flake-primops.cc`), transcribed rather than reinvented, and the
/// line it stops at is the line the charter draws: everything here is IO and
/// policy -- parsing the reference, the pure-eval rule, the registry, the
/// input-graph walk, the fetches -- and `callFlake` itself is an ordinary Nix
/// application the VM performs. What crosses back is a lock file and an
/// overrides document, i.e. data, exactly as it does for the `<flake>#attr`
/// command line.
///
/// **One seam, two ways in.** `rustEvaluandOf` builds the same three
/// arguments for the command line. Both call `flake::callFlakeSource()` and
/// `flakeOverridesJSON`, so a change to either reaches both, which is the
/// property `rust-flake-entry.md` asks for and the reason `getFlake` is not a
/// second implementation of flake evaluation.
///
/// The one difference from the command line is the `LockFlags`, and it is
/// cppnix's difference rather than this backend's: `prim_getFlake` never
/// updates or writes a lock file and decides `useRegistries` and
/// `allowUnlocked` off `pureEval`, where the command line takes the flags the
/// user's command line built.
static int rustLockFlake(
    void * ctx, const unsigned char * flakeRefPtr, size_t flakeRefLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.lockFlakeAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.lockFlakeAnswer.data());
        *outLen = host.lockFlakeAnswer.size();
        return rc;
    };

    /* The same refusal `rustFetchTree` makes, for the same reason and in the
       same words: `emitTreeAttrs` wraps every metadata attribute in a
       per-attribute recording thunk when the tracker is on, and
       `flakeOverridesJSON` below forces every one of them. Serialising them
       would record reads the flake never made and hand the evaluator plain
       values that can never record the ones it does. Both are silent, so a
       tracked evaluation gets a named refusal instead. */
    if (host.state.readSetTracker)
        return answer(
            "builtins.getFlake while the read-set tracker is on (the overrides this hands over "
            "are cppnix's emitTreeAttrs sets, which are per-attribute recording thunks under the "
            "tracker, and serialising them would both record reads nobody made and lose the ones "
            "the flake does make)",
            2);

    // As in `rustFetchTree`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        auto & state = host.state;
        std::string flakeRefS(reinterpret_cast<const char *>(flakeRefPtr), flakeRefLen);
        auto flakeRef = parseFlakeRef(state.fetchSettings, flakeRefS, {}, true);

        /* cppnix's own rule, raised here so the message is cppnix's. The
           position it prints is the one thing that cannot be transcribed: this
           backend has no positions yet (ENG-12137), so the `at %s` clause is
           dropped rather than filled with a made-up one. */
        if (state.settings.pureEval && !flakeRef.input.isLocked(state.fetchSettings))
            return answer(
                fmt("cannot call 'getFlake' on unlocked flake reference '%s' (use '--impure' to override)", flakeRefS),
                1);

        /* The ix-local backwards-compatibility branch `prim_getFlake` carries,
           kept in step with it: a lazily mounted store path has to be
           materialised before the path-input fetch can read it from disk. */
        if (auto sourcePath = flakeRef.input.getSourcePath();
            flakeRef.input.getType() == "path" && sourcePath && state.store->isInStore(sourcePath->string())) {
            auto [storePath, subPath] = state.store->toStorePath(sourcePath->string());
            state.ensureLazyPathCopied(storePath);
        }

        /* The one place the C++ evaluator serves under `eval-backend = rust`,
           and it is the same place the command line uses: `lockFlake`
           evaluates `flake.nix` to read its `inputs`. Scoped and counted as
           `evaluatorCalls.cppFlakeLock` so a `getFlake` run still reports
           `evaluator: rust`, and so the parity claim stays bounded to the two
           backends' reading of `outputs`. */
        std::shared_ptr<flake::LockedFlake> lockedFlake;
        {
            EvalState::LockingFlake locking(state);
            lockedFlake = std::make_shared<flake::LockedFlake>(flake::lockFlake(
                flakeSettings,
                state,
                flakeRef,
                flake::LockFlags{
                    .updateLockFile = false,
                    .writeLockFile = false,
                    .useRegistries = !state.settings.pureEval && flakeSettings.useRegistries,
                    .allowUnlocked = !state.settings.pureEval,
                }));
        }

        auto [lockFileStr, keyMap] = lockedFlake->lockFile.to_string();
        (void) keyMap;

        nlohmann::json doc = nlohmann::json::object({
            {"source", std::string(flake::callFlakeSource())},
            {"lockFile", lockFileStr},
            // A string holding a document, not a nested object: see the note
            // beside `ixe_lock_flake_fn` in ixe.h. A read set digests these
            // bytes, and a re-serialisation on the far side would put key
            // ordering between what was produced and what is digested.
            {"overrides", flakeOverridesJSON(state, *lockedFlake).dump()},
        });
        return answer(doc.dump(), 0);
    } catch (Error & e) {
        /* `message()`, like every other hook here, and the reason is at the
           first of them: the Rust arm carries no source positions (ENG-12137),
           so a trace block would be the only difference between the two arms'
           error text. `what()` additionally renders the whole `ErrorInfo`,
           "error: " prefix and all, on top of the prefix the evaluator adds --
           a `MissingExperimentalFeature` through here reads `error: error:
           experimental Nix feature 'flakes' is disabled`.

           This comment used to claim the other hooks had that bug and cite a
           ticket for sweeping them. They did not: every `catch (Error &)` in
           this file already used `message()`, and the `what()` calls a grep
           turns up are the `catch (std::exception &)` clauses beneath them,
           where `what()` is correct because a plain `std::exception` renders
           no prefix. The one defective site was this one, written against the
           pattern. `rust-nix-eval-gate.sh` now asserts the prefix appears
           exactly once, which is what would have caught it (ENG-13022). */
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.parseFlakeRef`: the flake-ref grammar, which lives here because
/// it is cppnix's `parseFlakeRef` and nothing else -- URL and path syntax,
/// registry shorthands, the attrs each scheme admits. The evaluator sends the
/// reference string and receives the parsed reference as a flat JSON object
/// (`toAttrs()` through `fetchers::attrsToJSON`, scalars only), which it
/// turns back into the attribute set the program sees.
///
/// The `flakes` experimental-feature check runs here and not in the
/// evaluator, mirroring where cppnix makes it: `flake-primops.cc` registers
/// the primop unconditionally with `.experimentalFeature = Xp::Flakes`, so
/// `builtins ? parseFlakeRef` is `true` with the feature off and only a call
/// raises `MissingExperimentalFeature`. `require` throws that same error, and
/// it travels back as an ordinary failure with cppnix's message.
///
/// One ordering edge is accepted rather than mirrored: cppnix's stub raises
/// before the argument is forced, where the evaluator forces first and this
/// gate runs only when the question arrives, so "feature off AND argument
/// invalid" reports the argument on the rust arm. See `bi_parse_flake_ref`
/// in `primops_host.rs` for why that is left alone.
static int rustParseFlakeRef(
    void * ctx, const unsigned char * flakeRefPtr, size_t flakeRefLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.parseFlakeRefAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.parseFlakeRefAnswer.data());
        *outLen = host.parseFlakeRefAnswer.size();
        return rc;
    };

    // As everywhere in this file: a C++ exception must not unwind through
    // Rust frames, so every failure comes back as a status and a message.
    try {
        experimentalFeatureSettings.require(Xp::Flakes);
        std::string flakeRefS(reinterpret_cast<const char *>(flakeRefPtr), flakeRefLen);
        // cppnix's own call, arguments and all (`prim_parseFlakeRef`,
        // flake-primops.cc:103): no base directory, `allowMissing = true`.
        auto attrs = parseFlakeRef(host.state.fetchSettings, flakeRefS, {}, true).toAttrs();
        return answer(fetchers::attrsToJSON(attrs).dump(), 0);
    } catch (Error & e) {
        // `message()`, not `what()`; the reason is written out at
        // `rustLockFlake`'s catch clause.
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.flakeRefToString`, the grammar's other direction:
/// `FlakeRef::fromAttrs(...).to_string()`. The evaluator has already forced
/// the set and raised cppnix's negative-integer and wrong-type errors on its
/// side, so what arrives is a bag of scalars -- `ixe_fetch_tree_fn`'s triplet
/// encoding without its leading fetcher field.
///
/// Feature gate here for the same reason as `rustParseFlakeRef` above.
static int rustFlakeRefToString(
    void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.flakeRefToStringAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.flakeRefToStringAnswer.data());
        *outLen = host.flakeRefToStringAnswer.size();
        return rc;
    };

    try {
        experimentalFeatureSettings.require(Xp::Flakes);

        // NUL-terminated fields, the encoding documented beside
        // `ixe_flake_ref_to_string_fn` in ixe.h.
        std::vector<std::string_view> fields;
        std::string_view rest(reinterpret_cast<const char *>(request), requestLen);
        while (!rest.empty()) {
            auto end = rest.find('\0');
            if (end == std::string_view::npos)
                return answer("rust-eval: unterminated field in flake ref request", 1);
            fields.emplace_back(rest.substr(0, end));
            rest.remove_prefix(end + 1);
        }
        if (fields.size() % 3 != 0)
            return answer("rust-eval: flake ref request has a partial attribute", 1);

        fetchers::Attrs attrs;
        for (size_t i = 0; i < fields.size(); i += 3) {
            std::string name(fields[i]);
            auto tag = fields[i + 1];
            std::string text(fields[i + 2]);
            if (tag == "s") {
                attrs.emplace(name, text);
            } else if (tag == "b") {
                attrs.emplace(name, Explicit<bool>{text == "1"});
            } else if (tag == "i") {
                attrs.emplace(name, static_cast<uint64_t>(std::stoull(text)));
            } else
                return answer("rust-eval: unknown attribute tag in flake ref request", 1);
        }

        return answer(FlakeRef::fromAttrs(host.state.fetchSettings, attrs).to_string(), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.fetchTree` and `builtins.fetchGit`, from `Input::fromAttrs`
/// onward.
///
/// The evaluator forced and classified the input attributes and raised the
/// errors a program can see; what arrives is the bag. Everything here is how
/// an `Input` is built and fetched, and it is cppnix's own code path in
/// cppnix's own order: `fixGitURL`, the `exportIgnore` and `shallow`
/// defaults, `Input::fromAttrs`, the registry, the locked-input check,
/// `checkURI`, the input cache, `mountInput` and `emitTreeAttrs`.
///
/// The answer is JSON rather than a store path because the attribute set has
/// no fixed shape. Each attribute's value is rendered by `printValueAsJSON`,
/// so there is one serialiser rather than a second hand-written one -- but
/// the set is assembled here rather than passed to it whole; see the comment
/// at the loop for what that avoids.
static int
rustFetchTree(void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.fetchTreeAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.fetchTreeAnswer.data());
        *outLen = host.fetchTreeAnswer.size();
        return rc;
    };

    /* The one thing this cannot serve, and it must say so rather than serve it
       wrong. `emitTreeAttrs` wraps every metadata attribute in a per-attribute
       recording thunk when the tracker is on (`allocRecordedTreeAttr`,
       primops/fetchTree.cc:41), so that an entry which never reads the
       revision does not acquire it as an input. Two things go wrong if this
       proceeds: the JSON below FORCES every one of those thunks, recording
       reads the program never made, and the evaluator receives plain values
       that can never record the reads it does make. Both are silent. So a
       tracked evaluation gets a named refusal instead. */
    if (host.state.readSetTracker)
        return answer(
            "the read-set tracker is on, and a tree fetch cannot carry its per-attribute "
            "recording through this backend: cppnix returns a thunk per metadata attribute "
            "that records the read when it is forced, and serialising them here would both "
            "force reads nobody made and lose the ones they do",
            2);

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        // NUL-terminated fields, the encoding documented beside
        // `ixe_fetch_tree_fn` in ixe.h.
        std::vector<std::string_view> fields;
        std::string_view rest(reinterpret_cast<const char *>(request), requestLen);
        while (!rest.empty()) {
            auto end = rest.find('\0');
            if (end == std::string_view::npos)
                return answer("rust-eval: unterminated field in tree fetch request", 1);
            fields.emplace_back(rest.substr(0, end));
            rest.remove_prefix(end + 1);
        }
        if (fields.empty())
            return answer("rust-eval: empty tree fetch request", 1);

        bool isFetchGit = false;
        bool isFinal = false;
        if (fields[0] == "fetchGit")
            isFetchGit = true;
        else if (fields[0] == "fetchFinalTree")
            isFinal = true;
        else if (fields[0] != "fetchTree")
            return answer("rust-eval: unknown fetcher in tree fetch request", 1);
        // cppnix's `fetcher` local, which is derived from `isFetchGit` alone
        // (`fetchTree.cc:186`), so a final fetch reports itself as
        // `fetchTree` in every message. The wire spelling and the message
        // spelling differ on purpose; see `TreeFetcher::error_name`.
        auto fetcher = std::string(isFetchGit ? "fetchGit" : "fetchTree");

        if ((fields.size() - 1) % 3 != 0)
            return answer("rust-eval: tree fetch request has a partial attribute", 1);

        fetchers::Attrs attrs;
        for (size_t i = 1; i < fields.size(); i += 3) {
            std::string name(fields[i]);
            auto tag = fields[i + 1];
            std::string text(fields[i + 2]);
            if (tag == "s") {
                // fixGitURL lives here and not in the evaluator: it is URL
                // parsing and re-rendering, it decides a store path, and
                // `GitInputScheme::fromAttrs` applies it too (git.cc:493).
                attrs.emplace(name, isFetchGit && name == "url" ? fixGitURL(text).to_string() : text);
            } else if (tag == "b") {
                attrs.emplace(name, Explicit<bool>{text == "1"});
            } else if (tag == "i") {
                attrs.emplace(name, static_cast<uint64_t>(std::stoull(text)));
            } else
                return answer("rust-eval: unknown attribute tag in tree fetch request", 1);
        }

        // cppnix's two default injections, kept on this side with
        // Input::fromAttrs because they are how the input is built.
        if (isFetchGit && !attrs.contains("exportIgnore")
            && (!attrs.contains("submodules") || !*fetchers::maybeGetBoolAttr(attrs, "submodules"))) {
            attrs.emplace("exportIgnore", Explicit<bool>{true});
        }
        auto type = fetchers::maybeGetStrAttr(attrs, "type");
        if (type == "git" && !isFetchGit && !attrs.contains("shallow")
            && !fetchers::maybeGetBoolAttr(attrs, "exportHistory").value_or(false)) {
            attrs.emplace("shallow", Explicit<bool>{true});
        }

        auto input = fetchers::Input::fromAttrs(host.state.fetchSettings, std::move(attrs));

        auto & state = host.state;
        if (!state.settings.pureEval && !input.isDirect() && experimentalFeatureSettings.isEnabled(Xp::Flakes))
            input =
                lookupInRegistries(state.fetchSettings, *state.store, input, fetchers::UseRegistries::Limited).first;

        if (state.settings.pureEval && !input.isLocked(state.fetchSettings)) {
            if (input.getNarHash())
                warn(
                    "Input '%s' is unlocked (e.g. lacks a Git revision) but is checked by NAR hash. "
                    "This is not reproducible and will break after garbage collection or when shared.",
                    input.to_string());
            else
                return answer(
                    fmt("in pure evaluation mode, '%s' doesn't fetch unlocked input '%s'", fetcher, input.to_string()),
                    1);
        }

        state.checkURI(input.toURLString());

        // cppnix's `params.isFinal` branch. A final fetch marks the input;
        // a plain one rejects an input that already carries the mark.
        if (isFinal)
            input.attrs.insert_or_assign("__final", Explicit<bool>(true));
        else if (input.isFinal())
            return answer(fmt("input '%s' is not allowed to use the '__final' attribute", input.to_string()), 1);

        auto cachedInput =
            state.inputCache->getAccessor(state.fetchSettings, *state.store, input, fetchers::UseRegistries::No);
        auto storePath = state.mountInput(cachedInput.lockedInput, input, cachedInput.accessor);

        Value v;
        // `emptyRevFallback` is fetchGit's, which is why the fetcher travels.
        emitTreeAttrs(state, storePath, cachedInput.lockedInput, v, isFetchGit, false);

        /* Attribute by attribute, NOT `printValueAsJSON` over the whole set.
           That function collapses any attrset carrying an `outPath` to that
           string alone (value-to-json.cc:100) -- the derivation shorthand --
           so serialising the set as a unit answers with a bare JSON string and
           loses every other attribute. It did exactly that on the first run:
           26 of 36 gate cases failed with "a fetched tree did not answer with
           an attribute set", which is the evaluator refusing to guess rather
           than accepting a string where a set belongs.

           Per attribute is immune because the shorthand fires on a *set* that
           has an `outPath` member, and these values are scalars and, for
           `history`, a set that has no such member. */
        state.forceAttrs(v, noPos, "while serialising a fetched tree");
        nlohmann::json treeJson = nlohmann::json::object();
        for (auto & a : *v.attrs()) {
            NixStringContext context;
            treeJson[std::string(state.symbols[a.name])] =
                printValueAsJSON(state, true, *a.value, noPos, context, false);
        }
        return answer(treeJson.dump(), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// cppnix's four spellings for a directory entry type, which is what
/// `builtins.readDir` and `builtins.readFileType` return (`primops.cc:2480`).
/// One copy, used by both hooks below, because two would be two chances to
/// disagree with the decoder on the Rust side.
///
/// Not `SourceAccessor::Stat::typeString`, which is a *different* mapping for
/// a different audience: it spells the exotic node types out ("character
/// device", "fifo") for a diagnostic, where `fileTypeToString` folds all of
/// them into "unknown" because that is the value a Nix program sees. Using
/// the diagnostic one here would make `builtins.readFileType "/dev/null"`
/// answer "character device" on this backend and "unknown" on cppnix.
static std::string_view fileTypeName(SourceAccessor::Type type)
{
    // Every enumerator named rather than a `default`, which is what
    // `-Werror=switch-enum` asks for and is worth having here: cppnix's
    // `fileTypeToString` folds the exotic node types into "unknown" behind a
    // `default`, so a node type added upstream would silently become
    // "unknown" on both arms. Naming them makes the next one stop this file
    // compiling until somebody decides, which is the louder half of the same
    // answer.
    switch (type) {
    case SourceAccessor::tRegular:
        return "regular";
    case SourceAccessor::tDirectory:
        return "directory";
    case SourceAccessor::tSymlink:
        return "symlink";
    // What `fileTypeToString` answers for all of these: "unknown" is the
    // value a Nix program sees for a device node, a socket or a fifo.
    case SourceAccessor::tChar:
    case SourceAccessor::tBlock:
    case SourceAccessor::tSocket:
    case SourceAccessor::tFifo:
    case SourceAccessor::tUnknown:
        return "unknown";
    }
    // Unreachable for a well-formed value, and required: a `switch` over an
    // enum is not a total function in C++, and falling off the end without
    // returning is undefined behaviour rather than a compile error.
    return "unknown";
}

/// `builtins.readFile`, and the bytes half of an `import`.
///
/// Here rather than in the evaluator because `pure-eval` and `restrict-eval`
/// are enforced by the accessor: cppnix wraps `rootFS` in an
/// `AllowListSourceAccessor` when either is set (`eval.cc:306`), so a read
/// that does not go through `rootFS` cannot honour them. The evaluator's own
/// `std::fs` reader consults no allow list, so before this hook existed it
/// refused all five plain reads under either setting rather than answering
/// outside the list -- which made no flake evaluable on this backend, since
/// flake entry means importing files out of a fetched store path. ENG-12792.
///
/// A transcription of `prim_readFile` (`primops.cc:2201`) from `realisePath`
/// onward: the evaluator has already coerced the value to a path and realised
/// its context, so what is left is the resolution, `rootPath` and the read.
/// Nothing here re-decides which path to read.
///
/// `resolveSymlinks` is the part that is easy to drop and was
/// (ENG-12871). `realisePath`'s `resolveSymlinks` argument defaults to
/// `SymlinkResolution::Full` (`eval.hh:1133`) and `prim_readFile` takes the
/// default, so this is `Full` too. It is not optional politeness:
/// `PosixSourceAccessor::readFile` opens with `O_NOFOLLOW` behind an
/// `assertNoSymlinks` over the whole path (`posix-source-accessor.cc:42`), so
/// without the resolution a symlink is an error rather than a slower read.
/// cppnix puts symlink following in `EvalState::realisePath` on purpose and
/// keeps the accessor strict, which means every caller has to say what it
/// wants -- and the resolution runs *through this accessor*, so `pure-eval`
/// and `restrict-eval` apply to each component it walks exactly as they do in
/// cppnix.
///
/// `prim_readFile`'s reference scan is deliberately not transcribed. It gives
/// the resulting *string* the store references found in the bytes, and this
/// boundary carries bytes and not contexts; the evaluator's own
/// `NeedPath::Contents` answer is a plain string on both arms today, so
/// adding half of the scan here would be a divergence rather than a fix. It
/// belongs with the read set's own context work (ENG-12465).
static int
rustReadFile(void * ctx, const unsigned char * path, size_t pathLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.readFileAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.readFileAnswer.data());
        *outLen = host.readFileAnswer.size();
        return rc;
    };

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message. A
    // RestrictedPathError arrives here like any other Error and goes back as
    // its own text, which is right: cppnix does not make one catchable
    // either, since `prim_tryEval` catches `AssertionError` only
    // (`primops.cc:1219`).
    try {
        std::string p(reinterpret_cast<const char *>(path), pathLen);
        return answer(host.state.rootPath(CanonPath(p)).resolveSymlinks(SymlinkResolution::Full).readFile(), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.pathExists`.
///
/// A transcription of `prim_pathExists` (`primops.cc:2081`), and the catch is
/// the whole of it: cppnix turns a forbidden path into `false` rather than a
/// failure (`primops.cc:2097`), and a missing one into `false` through
/// `maybeLstat`. So this answers a plain Boolean and has no error channel,
/// which is also why the evaluator's `Host::path_exists` returns a bare
/// `bool`.
///
/// Every other exception reads as `false` too. That is not cppnix's rule --
/// there an IO error propagates -- and it is the same answer the `std::fs`
/// reader this replaces gave, which swallowed every error into `false`.
/// Narrowing it means giving `Host::path_exists` a failure case, which is a
/// change to every host in the crate; filed rather than half-done.
///
/// The trailing-slash branch of `prim_pathExists` is not transcribed because
/// it cannot be reached from here: it inspects the *value*
/// (`primops.cc:2088`), and only a string argument can end in `/`. The
/// evaluator has already coerced to a path, and `CanonPath` has no trailing
/// slash to inspect. That branch is also the only one that would resolve
/// `Full`; every path that reaches here takes `prim_pathExists`'s other
/// branch, which is `Ancestors`.
///
/// `Ancestors` and not `Full`, and the difference is the whole answer for a
/// dangling symlink: `Full` would resolve the link to its missing target and
/// report `false`, where cppnix leaves the last component alone, `lstat`s the
/// link itself and reports `true`. Resolving nothing at all is wrong the
/// other way -- `maybeLstat` runs `assertNoSymlinks` over the *parent*
/// (`posix-source-accessor.cc:96`), so a path with a symlinked ancestor threw
/// where cppnix answers `true`, and this hook swallowed that into `false`.
/// ENG-12871.
static int rustPathExists(void * ctx, const unsigned char * path, size_t pathLen)
{
    auto & host = hostOf(ctx);
    try {
        std::string p(reinterpret_cast<const char *>(path), pathLen);
        auto st = host.state.rootPath(CanonPath(p)).resolveSymlinks(SymlinkResolution::Ancestors).maybeLstat();
        return st ? 1 : 0;
    } catch (...) {
        return 0;
    }
}

/// `builtins.readDir`. A transcription of `prim_readDir` (`primops.cc:2508`)
/// from `realisePath` onward, `SymlinkResolution::Full` like
/// `prim_readFile`'s.
///
/// Resolving first also puts the right path in the error text for a symlink
/// to a non-directory: `PosixSourceAccessor::readDirectory` names the path it
/// opened, so cppnix says `cannot read directory ".../foo/git-hates-
/// directories"` where the expression named `.../linked`. That is
/// `eval-fail-readDir-not-a-directory-2`, and it is why skipping the
/// resolution showed up as an error-class mismatch and not only as two
/// refused values.
///
/// One difference, and it is forced by the boundary: `prim_readDir` leaves an
/// entry whose type the filesystem did not report as a thunk that calls
/// `builtins.readFileType` when something forces it. There is no lazy field
/// in a NUL-separated buffer, so the type is resolved here instead. An entry
/// that cannot be stat'ed reads as "unknown" rather than failing the whole
/// listing, because the lazy version would only have failed if the program
/// looked at that one entry, and failing the directory would be stricter than
/// cppnix rather than equal to it.
static int
rustReadDir(void * ctx, const unsigned char * path, size_t pathLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.readDirAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.readDirAnswer.data());
        *outLen = host.readDirAnswer.size();
        return rc;
    };

    // As in `rustReadFile`.
    try {
        std::string p(reinterpret_cast<const char *>(path), pathLen);
        auto dir = host.state.rootPath(CanonPath(p)).resolveSymlinks(SymlinkResolution::Full);
        std::string encoded;
        for (auto & [name, maybeType] : dir.readDirectory()) {
            auto type = maybeType;
            if (!type) {
                // What the lazy `builtins.readFileType` thunk would have
                // done, eagerly.
                if (auto st = (dir / name).maybeLstat())
                    type = st->type;
            }
            encoded.append(name);
            encoded.push_back('\0');
            encoded.append(fileTypeName(type.value_or(SourceAccessor::tUnknown)));
            encoded.push_back('\0');
        }
        return answer(encoded, 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// The non-resolving kind query: `maybeLstat`, not `stat`, so a symlink reads
/// as a symlink and a path the accessor has no answer for reads as `absent`.
///
/// A transcription of the accessor call under `prim_readFileType`
/// (`primops.cc:2490`), which is the one read hook that must NOT resolve and
/// the only primop in the family that passes `std::nullopt` to `realisePath`
/// (`primops.cc:2492`). Adding a `resolveSymlinks` here to match its three
/// siblings would be a regression, not a tidy-up: it would answer
/// `"regular"` where cppnix reports the symlink, and it would answer at all
/// on a path with a symlinked ancestor, where cppnix raises
/// `SymlinkNotAllowed` from `maybeLstat`'s `assertNoSymlinks` and so must
/// this. Both halves are pinned by corpus pairs; see
/// `eval-okay-readFileType-symlink` and
/// `eval-fail-readFileType-symlinked-ancestor`.
///
/// # `maybeLstat` and not `lstat`, which is a semantic change and deliberate
///
/// `SourceAccessor::lstat` is `maybeLstat` plus `throw FileNotFound`
/// (`source-accessor.cc:73`), and throwing here made the bridge decide
/// something it has no standing to decide. Two callers ask this question and
/// they want opposite things from a missing path: `builtins.readFileType`
/// wants cppnix's error, and the ancestor scan in `builtins.path`'s filter
/// walk wants what cppnix's `resolveSymlinks` gets from the same accessor
/// call (`source-accessor.cc:91`) -- nullopt, meaning "not a symlink",
/// recorded as the observation `absent`.
///
/// Under pure eval that distinction is the whole ball game. `rootFS` is then
/// a mounted accessor holding `/` -> empty and `/nix/store` -> the store
/// (`eval.cc:294`), so `/nix` -- an ancestor of every store path -- has no
/// mount, falls to the empty accessor and lstats as missing. With the throw
/// here, every filtered `builtins.path` under pure eval died on
/// `path '/nix' does not exist`: 90 of ix's 144 flake attributes, none of
/// which cppnix has any trouble with. ENG-13123.
///
/// So this hands nullopt over as `absent` and the evaluator decides which
/// caller gets an error, which is where the decision belongs.
///
/// `absent` is the only answer that moved. A `RestrictedPathError`, a
/// `SymlinkNotAllowed` or a broken directory is still a non-zero return with
/// its text, because those are the accessor refusing or failing rather than
/// answering, and folding one of them into `absent` would report a forbidden
/// path as an ordinary missing one.
///
/// The half of an `import` that decides whether a path names a directory is
/// `rustFileTypeResolved` below and not this one, because cppnix's `import`
/// resolves where its `readFileType` does not.
static int
rustFileType(void * ctx, const unsigned char * path, size_t pathLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](std::string_view text, int rc) {
        host.fileTypeAnswer = std::string(text);
        *out = reinterpret_cast<const unsigned char *>(host.fileTypeAnswer.data());
        *outLen = host.fileTypeAnswer.size();
        return rc;
    };

    // As in `rustReadFile`.
    try {
        std::string p(reinterpret_cast<const char *>(path), pathLen);
        auto st = host.state.rootPath(CanonPath(p)).maybeLstat();
        return answer(st ? fileTypeName(st->type) : "absent", 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// The half of an `import` that decides whether a path names a directory and
/// so imports its `default.nix`.
///
/// A transcription of `resolveExprPath` (`eval.cc:3423`), which is where
/// cppnix's `import` does its symlink resolution: `prim_import` passes
/// `std::nullopt` to `realisePath` (`primops.cc:300`) exactly like
/// `prim_readFileType` does, and then `resolveExprPath` resolves anyway. Its
/// directory test is `path.resolveSymlinks().lstat().type == tDirectory`
/// (`eval.cc:3440`), which is `Full` and then the type, so that is what this
/// is.
///
/// Only the directory test is here. `resolveExprPath` also rewrites the path
/// to the symlink's target so a relative import inside the imported file
/// resolves against the target's directory; that is a decision about which
/// path the evaluator then reads, it belongs in the evaluator with the
/// `/default.nix` append it sits beside (`Host::resolve_import`), and this
/// hook answers only the question the world can answer. The two are
/// distinguishable only where the link and its target are different
/// directories with different contents; ENG-12914 tracks it.
///
/// Sharing one hook with `rustFileType` was ENG-12871: `import
/// a/symlinked-dir/f.nix` came back as "path 'a/symlinked-dir' is a symlink"
/// because `lstat`'s `assertNoSymlinks` refuses a symlinked ancestor, which
/// is right for `builtins.readFileType` and wrong for an `import`.
static int rustFileTypeResolved(
    void * ctx, const unsigned char * path, size_t pathLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](std::string_view text, int rc) {
        host.fileTypeResolvedAnswer = std::string(text);
        *out = reinterpret_cast<const unsigned char *>(host.fileTypeResolvedAnswer.data());
        *outLen = host.fileTypeResolvedAnswer.size();
        return rc;
    };

    // As in `rustReadFile`.
    try {
        std::string p(reinterpret_cast<const char *>(path), pathLen);
        return answer(
            fileTypeName(host.state.rootPath(CanonPath(p)).resolveSymlinks(SymlinkResolution::Full).lstat().type), 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.appendContext` makes every key it is handed present before
/// putting it in a string's context (context.cc:270) -- unless read-only mode
/// is on, in which case cppnix skips the call and so does this. That branch
/// lives here rather than in the evaluator because `settings.readOnlyMode` is
/// ours. ENG-12479.
static int
rustEnsurePath(void * ctx, const unsigned char * path, size_t pathLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto fail = [&](const std::string & text) {
        host.ensurePathAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.ensurePathAnswer.data());
        *outLen = host.ensurePathAnswer.size();
        return 1;
    };

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        std::string p(reinterpret_cast<const char *>(path), pathLen);
        if (!settings.readOnlyMode)
            host.state.store->ensurePath(host.state.store->parseStorePath(p));
        return 0;
    } catch (Error & e) {
        return fail(e.message());
    } catch (std::exception & e) {
        return fail(e.what());
    }
}

/// Import from derivation: make a string context's derivation outputs valid
/// so the evaluator can read through them.
///
/// This calls `EvalState::realiseContext` rather than transcribing it, which
/// is the point of the hook. Every branch that decides what IFD means --
/// `isValidPath` on each element and its program-visible `InvalidPathError`,
/// `allow-import-from-derivation` and its `IFDError`,
/// `trace-import-from-derivation`, `buildPaths`, `resolveDerivedPath`, the
/// `copyClosure` when evaluation and build stores differ, and `allowClosure`
/// on the outputs -- is a `settings` or a store branch that the evaluator
/// cannot see and must not guess at. A second copy of that logic here is
/// exactly the pair of implementations this boundary exists to avoid, and it
/// would drift silently: the two arms would agree on every derivation that
/// builds and disagree only on the ones that do not.
///
/// It also means `readSetTracker->recordStoreQuery` inside `ensureValid`
/// still runs, so a cpp-arm read set covers the same store queries the rust
/// arm records against its own `Question::Realise`.
///
/// `isIFD` is true, not defaulted: every caller on the Rust side is a
/// read-shaped builtin, which is what that flag means. Passing false would
/// quietly disable the `allow-import-from-derivation` check.
/// Decode the NUL-terminated context elements every realise hook receives.
/// Returns nullopt for a malformed request, with the reason in `why`; one
/// copy of the parse, because three hooks now read the same encoding and a
/// drifted copy would make the checked context and the built context
/// different sets.
static std::optional<NixStringContext>
decodeRealiseRequest(const unsigned char * request, size_t requestLen, std::string & why)
{
    NixStringContext context;
    std::string_view rest(reinterpret_cast<const char *>(request), requestLen);
    while (!rest.empty()) {
        auto end = rest.find('\0');
        if (end == std::string_view::npos) {
            why = "rust-eval: unterminated element in realise request";
            return std::nullopt;
        }
        // `parse` and not a hand-rolled split on '!' and '=': the
        // evaluator rendered these with cppnix's own spelling, so they
        // come back through cppnix's own reader.
        context.insert(NixStringContextElem::parse(rest.substr(0, end)));
        rest.remove_prefix(end + 1);
    }
    if (context.empty()) {
        why = "rust-eval: realise request carries no context";
        return std::nullopt;
    }
    return context;
}

static int
rustRealise(void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.realiseAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.realiseAnswer.data());
        *outLen = host.realiseAnswer.size();
        return rc;
    };

    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames, so every failure comes back as a status and a message.
    try {
        std::string why;
        auto context = decodeRealiseRequest(request, requestLen, why);
        if (!context)
            return answer(why, 1);

        auto rewrites = host.state.realiseContext(*context);

        // NUL-terminated from/to pairs. Empty for every input-addressed
        // derivation, which is the common case and not an error.
        std::string encoded;
        for (auto & [from, to] : rewrites) {
            encoded.append(from);
            encoded.push_back('\0');
            encoded.append(to);
            encoded.push_back('\0');
        }
        return answer(encoded, 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// Phase 1 of a non-blocking import from derivation: everything
/// `EvalState::realiseContext` does before the build that touches this
/// evaluator's own state. Runs on the evaluation thread, from the Rust
/// host's `begin`, before the build is handed to a worker: the `isValidPath`
/// checks land in `readSetTracker` (a plain map with no lock), and the
/// `allow-import-from-derivation` refusal must be decided before anything is
/// spawned for it.
///
/// A non-zero status here means the Rust side declines to begin and falls
/// back to the synchronous `rustRealise`, which re-runs these checks and
/// reports the same failure through the same path the blocking flow always
/// used -- so the message text and its catchability cannot drift between the
/// two flows. The message written here is therefore never program-visible;
/// it exists for debugging.
static int rustRealiseCheck(
    void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.realiseCheckAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.realiseCheckAnswer.data());
        *outLen = host.realiseCheckAnswer.size();
        return rc;
    };

    try {
        std::string why;
        auto context = decodeRealiseRequest(request, requestLen, why);
        if (!context)
            return answer(why, 1);
        auto drvs = host.state.realiseContextCheck(*context, nullptr, true);
        // Nothing to build is not an error, but it is also nothing worth a
        // thread: the caller should take the synchronous path, whose
        // empty-drvs case returns the empty map without building.
        if (drvs.empty())
            return answer("rust-eval: realise context has nothing to build", 1);
        return answer("", 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// Phase 2: the build, called from a worker thread the Rust host owns --
/// the one hook in this vtable with that contract, spelled out beside
/// `ixe_realise_build_fn` in ixe.h. `EvalState::realiseContextBuild` touches
/// the stores and nothing else of the `EvalState` (the property its
/// declaration documents); the stores serve concurrent callers. Everything
/// this hook itself touches must respect the same rule, which is why its
/// answer buffer is `thread_local` rather than a member of `RustEvalHost`:
/// two builds in flight write two threads' buffers, and neither shares one
/// with the evaluation thread's hooks. The member buffers assume one caller
/// at a time and every other hook keeps that assumption.
///
/// Success writes the rewrite map, then an empty field as a separator, then
/// the output store paths phase 3 must register in the allow list -- all
/// NUL-terminated, unambiguous because neither a placeholder nor a store
/// path can be empty or contain a NUL.
static int rustRealiseBuild(
    void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    static thread_local std::string buildAnswer;
    auto answer = [&](const std::string & text, int rc) {
        buildAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(buildAnswer.data());
        *outLen = buildAnswer.size();
        return rc;
    };

    try {
        std::string why;
        auto context = decodeRealiseRequest(request, requestLen, why);
        if (!context)
            return answer(why, 1);

        // The build requests, reassembled from the same bytes phase 1
        // checked. Assembly only -- no validity checks, no read-set
        // recording, no settings refusals: those ran on the evaluation
        // thread in `rustRealiseCheck` and must not run here.
        std::vector<DerivedPath::Built> drvs;
        for (auto & c : *context)
            if (auto * b = std::get_if<NixStringContextElem::Built>(&c.raw))
                drvs.push_back(
                    DerivedPath::Built{
                        .drvPath = b->drvPath,
                        .outputs = OutputsSpec::Names{b->output},
                    });
        if (drvs.empty())
            return answer("rust-eval: realise build request has nothing to build", 1);

        StorePathSet outputsToAllow;
        auto rewrites = host.state.realiseContextBuild(drvs, nullptr, outputsToAllow);

        std::string encoded;
        for (auto & [from, to] : rewrites) {
            encoded.append(from);
            encoded.push_back('\0');
            encoded.append(to);
            encoded.push_back('\0');
        }
        // The separator: an empty field, which no `from` can be.
        encoded.push_back('\0');
        for (auto & p : outputsToAllow) {
            encoded.append(host.state.store->printStorePath(p));
            encoded.push_back('\0');
        }
        return answer(encoded, 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// Phase 3: register the built outputs in the evaluator's allow list, called
/// from the evaluation thread at the moment the answer is delivered. This is
/// the fix for the one structure the thread-safety audit found unsafe: the
/// allow list behind `EvalState::allowClosure` is a plain prefix set with no
/// lock, read by the evaluation thread on every file access, so the worker
/// must never touch it. Delivery order is the scheduler's token mint order,
/// so when this runs -- and therefore the order the allow list grows in --
/// is a property of the program, not of which build finished first.
///
/// `request` is the output paths from `rustRealiseBuild`'s answer, one
/// NUL-terminated store path per field.
static int rustRealiseAllow(
    void * ctx, const unsigned char * request, size_t requestLen, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.realiseAllowAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.realiseAllowAnswer.data());
        *outLen = host.realiseAllowAnswer.size();
        return rc;
    };

    try {
        std::string_view rest(reinterpret_cast<const char *>(request), requestLen);
        while (!rest.empty()) {
            auto end = rest.find('\0');
            if (end == std::string_view::npos)
                return answer("rust-eval: unterminated store path in realise allow request", 1);
            if (end > 0)
                host.state.allowClosure(host.state.store->parseStorePath(rest.substr(0, end)));
            rest.remove_prefix(end + 1);
        }
        return answer("", 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// The evaluator's warnings go to our logger, at our verbosity, formatted the
/// way every other cppnix warning is. Nothing to answer and nothing to fail:
/// a warning below the verbosity threshold is dropped here exactly as one
/// from `warn()` anywhere else in the process would be.
static void rustWarn(void *, const unsigned char * message, size_t messageLen)
{
    // The shadow arm's copy of a warning cppnix has already issued; see
    // `ShadowAttempt`.
    if (shadowAttempt.active)
        return;
    warn("%s", std::string_view(reinterpret_cast<const char *>(message), messageLen));
}

/// Split the NUL-separated prefix/path pairs the evaluator sends into
/// cppnix's own LookupPath. The encoding is documented beside
/// `ixe_find_file_fn` in ixe.h.
static LookupPath decodeLookupPath(const unsigned char * entries, size_t entriesLen)
{
    LookupPath lookupPath;
    std::string_view rest(reinterpret_cast<const char *>(entries), entriesLen);
    while (!rest.empty()) {
        auto cut = rest.find('\0');
        if (cut == rest.npos)
            break;
        std::string prefix(rest.substr(0, cut));
        rest.remove_prefix(cut + 1);
        cut = rest.find('\0');
        if (cut == rest.npos)
            break;
        std::string path(rest.substr(0, cut));
        rest.remove_prefix(cut + 1);
        lookupPath.elements.emplace_back(
            LookupPath::Elem{
                .prefix = LookupPath::Prefix{.s = prefix},
                .path = LookupPath::Path{.s = path},
            });
    }
    return lookupPath;
}

/// `<x>` and `builtins.findFile`. The evaluator hands over the list the
/// program actually passed -- which is `__nixPath` unless it rebound it --
/// and cppnix's own findFile does the resolving, because that reaches
/// fetchers, the corepkgs accessor and this evaluator's access control.
/// ENG-12443.
static int rustFindFile(
    void * ctx,
    const unsigned char * entries,
    size_t entriesLen,
    const unsigned char * name,
    size_t nameLen,
    const unsigned char ** out,
    size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.findFileAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.findFileAnswer.data());
        *outLen = host.findFileAnswer.size();
        return rc;
    };

    std::string sought(reinterpret_cast<const char *>(name), nameLen);
    // As in `rustCopyToStore`: a C++ exception must not unwind through Rust
    // frames. The ThrownError case is separated because cppnix raises a miss
    // as one and `builtins.tryEval` catches it, which a corpus case checks.
    try {
        auto path = host.state.findFile(decodeLookupPath(entries, entriesLen), sought);
        // A resolved path is only useful to the evaluator if the evaluator can
        // then read it, and it reads the real filesystem directly. cppnix can
        // resolve into accessors that are not the real one: `corepkgs` holds
        // `<nix/fetchurl.nix>` in memory, and a downloaded search path entry
        // lives behind the accessor its fetcher returned.
        //
        // Handing one of those back as an absolute path alone would name a file
        // that does not exist, which is why this used to refuse (ENG-12443).
        // Instead the bytes go over with it and the evaluator reads them from
        // memory, so the *path* stays the one cppnix reports and
        // `builtins.toString <nix/fetchurl.nix>` is `/fetchurl.nix` on both
        // arms rather than a store path on one. ENG-12607.
        //
        // Read here rather than lazily because the accessor is the evaluator's
        // and does not outlive it, and because `corepkgs` is one small file: a
        // lazy handle would buy nothing and would have to be kept alive.
        if (path.accessor != host.state.rootFS) {
            auto abs = path.path.abs();
            std::string contents;
            try {
                contents = path.readFile();
            } catch (Error & e) {
                // Resolved but unreadable. Refused rather than reported as a
                // miss: a miss is catchable and would send the caller down a
                // path cppnix never takes.
                return answer(
                    fmt("reading '<%s>' from an accessor that is not the real filesystem: %s", sought, e.message()), 2);
            }
            if (ixe_add_virtual_file(
                    reinterpret_cast<const unsigned char *>(abs.data()),
                    abs.size(),
                    reinterpret_cast<const unsigned char *>(contents.data()),
                    contents.size())
                != 0)
                return answer(fmt("registering '<%s>' with the evaluator", sought), 1);
            return answer(abs, 0);
        }
        return answer(path.path.abs(), 0);
    } catch (ThrownError & e) {
        return answer(e.message(), 5);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// `builtins.nixPath`: the -I flags and NIX_PATH this process was started
/// with, in the same encoding.
static int rustNixPath(void * ctx, const unsigned char ** out, size_t * outLen)
{
    auto & host = hostOf(ctx);
    auto answer = [&](const std::string & text, int rc) {
        host.nixPathAnswer = text;
        *out = reinterpret_cast<const unsigned char *>(host.nixPathAnswer.data());
        *outLen = host.nixPathAnswer.size();
        return rc;
    };

    try {
        std::string encoded;
        for (auto & e : host.state.getLookupPath().elements) {
            encoded.append(e.prefix.s);
            encoded.push_back('\0');
            encoded.append(e.path.s);
            encoded.push_back('\0');
        }
        return answer(encoded, 0);
    } catch (Error & e) {
        return answer(e.message(), 1);
    } catch (std::exception & e) {
        return answer(e.what(), 1);
    }
}

/// Whether the operator has asked this process to stop.
///
/// cppnix's signal handler thread sets the flag; `isInterrupted` is an atomic
/// load, which is what makes it safe to call from inside the VM's poll loop
/// rather than at a scheduler boundary. `checkInterrupt` is the usual
/// spelling and is wrong here: it throws, and a C++ exception must not unwind
/// through Rust frames. ENG-12533.
static int rustInterrupted(void *)
{
    if (isInterrupted())
        return 1;
    /* The shadow budget, as a deadline this attempt cannot run past. Only
       ever true inside a shadow: a served evaluation is the user's answer and
       must never be cut short by this harness's accounting. */
    if (shadowAttempt.active && shadowAttempt.deadline && shadowDeadlinePassed()) {
        shadowAttempt.tripped = true;
        return 1;
    }
    return 0;
}

/// `builtins.trace`. cppnix's own wording and its own sink: `printError`
/// with the `trace: ` prefix (`primops.cc:1325`), so the prefix has one copy
/// and it is this one.
static void rustTrace(void *, const unsigned char * message, size_t messageLen)
{
    // Likewise; see `ShadowAttempt`. A `builtins.trace` in nixpkgs fires on
    // both arms, and one deprecation notice printed twice is the served
    // command's output changing because shadow is on.
    if (shadowAttempt.active)
        return;
    printError("trace: %1%", std::string_view(reinterpret_cast<const char *>(message), messageLen));
}

/// Build the vtable once, pointing every entry at this object.
///
/// Every field is filled here rather than left for a caller to install, which
/// is the property that replaced fourteen `ixe_set_*` setters: a session
/// cannot be half configured, and a hook cannot arrive after an evaluation
/// has started under the assumption it was absent.
///
/// `rustFileType` and `rustFileTypeResolved` are two different functions on
/// purpose and putting the same one in both fields would be a bug: the first
/// is `builtins.readFileType`, which resolves nothing, and the second is the
/// directory test inside an `import`, which resolves everything. ENG-12871.
///
/// The five path reads are all present, which is what turns the last five
/// `Refuse` rows in `rust/nix-eval-rs/src/purity.rs` into served questions,
/// and therefore what lets a flake's files be imported out of a fetched store
/// path under pure eval. The evaluator refuses a partial set outright --
/// `ixe_session_new` returns null and `RustEvalSetup` throws -- because the
/// purity table decides those questions as a group: it can honour `pure-eval`
/// and `restrict-eval` for them only when every one goes through this
/// evaluator's `rootFS`. ENG-12792.
RustEvalHost::RustEvalHost(EvalState & state)
    : state(state)
    , vtable{
          .ctx = this,
          .copy_to_store = rustCopyToStore,
          .store_text = rustStoreText,
          .write_derivation = rustWriteDerivation,
          .store_filtered = rustStoreFiltered,
          .fetch = rustFetch,
          .fetch_tree = rustFetchTree,
          .lock_flake = rustLockFlake,
          .parse_flake_ref = rustParseFlakeRef,
          .flake_ref_to_string = rustFlakeRefToString,
          .ensure_path = rustEnsurePath,
          .realise = rustRealise,
          // The threaded import-from-derivation path (ENG-13150). Supplying
          // `realise_build` is this embedder's written consent that the
          // evaluator may call it from a worker thread; the check and allow
          // halves stay on the evaluation thread. All three plus `realise`
          // above, or the evaluator refuses the vtable.
          .realise_check = rustRealiseCheck,
          .realise_build = rustRealiseBuild,
          .realise_allow = rustRealiseAllow,
          .find_file = rustFindFile,
          .nix_path = rustNixPath,
          .warn = rustWarn,
          .trace = rustTrace,
          .interrupted = rustInterrupted,
          .read_file = rustReadFile,
          .path_exists = rustPathExists,
          .read_dir = rustReadDir,
          .file_type = rustFileType,
          .file_type_resolved = rustFileTypeResolved,
      }
{
}

/// nix-instantiate.cc's `OutputKind`, which is local to that file. Repeated
/// rather than shared because moving it into the header would put a
/// nix-instantiate detail in front of every other caller of this bridge; the
/// cost is that the two have to be changed together, and the values are
/// checked against it below.
enum OutputKindMirror { okPlain = 0, okRaw = 1, okXML = 2, okJSON = 3 };

/// Raise whatever the Rust side said when it refused a once-only setting.
///
/// These three settings are fixed for the lifetime of the process. Changing
/// one mid-process is an embedder bug, not a user error, and it used to be
/// ignored: an evaluator asked to serve a second store kept the first store's
/// directory and computed every path under it, silently (ENG-12541). Throwing
/// puts it in front of somebody the first time it happens.
static void setOnce(const char * what, int status)
{
    if (status == 0)
        return;
    // Taken and freed here rather than through the IxeString guard, which is
    // declared further down this file; one call, one free, no exit path
    // between them.
    char * raw = ixe_take_setting_conflict();
    std::string text = raw ? std::string(raw) : std::string();
    ixe_string_free(raw);
    if (text.empty())
        text = fmt("%s cannot be changed once the process has set it", what);
    throw Error("rust-eval: %s", text);
}

RustEvalSetup::RustEvalSetup(EvalState & state)
    : hostState(std::make_unique<RustEvalHost>(state))
{
    // Every crossing into nix-eval-rs constructs one of these, so this is the
    // one place that can count what the Rust backend served. The NIX_SHOW_STATS
    // `evaluator` field is derived from this count and the C++ one, rather
    // than echoing the setting that asked for it (ENG-12542).
    state.countRustEval();
    // The counters are per-evaluation, so they are zeroed here and read in
    // `~RustEvalSetup`. Without the reset a second evaluation in one process
    // reports the sum of both, which is a plausible-looking wrong number
    // rather than an obviously wrong one (ENG-12859).
    ixe_perf_reset();
    // cppnix bounds recursion with max-call-depth because its evaluator
    // recurses on the host stack. This VM keeps frames on the heap, so the
    // same program allocates instead of faulting and runs until the machine
    // gives out (ENG-12432); the limit has to be handed over explicitly for
    // the two arms to refuse the same programs.
    ixe_set_max_call_depth(state.settings.maxCallDepth);
    // One copy of the version number, ours, handed over rather than
    // duplicated in the Rust crate where it would drift.
    setOnce(
        "nixVersion",
        ixe_set_nix_version(reinterpret_cast<const unsigned char *>(nixVersion.data()), nixVersion.size()));
    // The platform, from the same setting cppnix's own builtins.currentSystem
    // reads, so `--system` moves both arms together.
    {
        const auto & system = state.settings.getCurrentSystem();
        setOnce(
            "currentSystem",
            ixe_set_current_system(reinterpret_cast<const unsigned char *>(system.data()), system.size()));
    }
    // The store directory goes into the fingerprint of every path
    // `builtins.derivationStrict` computes, so the Rust side is told rather
    // than left to assume: an assumed `/nix/store` against a store rooted
    // elsewhere is a wrong output path that looks exactly like a right one.
    {
        const auto & storeDir = state.store->storeDir;
        setOnce(
            "the store directory",
            ixe_set_store_dir(reinterpret_cast<const unsigned char *>(storeDir.data()), storeDir.size()));
    }
    // `~/x` is resolved by the compiler, not by a primop, so the Rust side
    // needs the home directory before it compiles anything. getHome() rather
    // than getenv("HOME") because that is the function the cppnix parser
    // calls (parser.y:467), and its $HOME is validated: unset or unowned
    // falls back to the passwd entry. Two implementations of that rule would
    // differ by naming a different file, so there is one and this hands over
    // its answer.
    //
    // Swallowing the throw is the whole point of the try. cppnix asks this
    // question lazily, from the parser, and only when a `~` literal is
    // actually parsed -- so a process with no `$HOME` and no passwd entry
    // (a container running as a uid nobody created) evaluates everything
    // except home paths perfectly well. Asking eagerly here to hand the
    // answer over would turn that into "every rust-eval evaluation fails",
    // which is a far larger blast radius than the construct deserves.
    // Leaving the setting unmade instead puts the failure back where cppnix
    // has it: the crate has no home directory, so a `~` literal reports
    // `getHomeOf`'s own words and nothing else changes.
    //
    // One divergence survives and is named rather than hidden. With nothing
    // set, the crate falls back to `$HOME` (see `Settings::current`), so the
    // sliver where `$HOME` is set but names a directory this euid does not
    // own AND there is no passwd entry gets that directory here and an error
    // from cppnix. It is left because the fallback is what makes the crate
    // usable standalone, and because the alternative -- handing over an
    // empty string -- resolves `~/x` to `/x`, which is a wrong file rather
    // than no file.
    try {
        const auto home = getHome().string();
        setOnce(
            "the home directory", ixe_set_home_dir(reinterpret_cast<const unsigned char *>(home.data()), home.size()));
    } catch (Error &) {
    }
    // Opt-in on-disk cache of compiled modules and evaluation results. Empty
    // means in-memory only, which is what every release so far has done, so
    // the default flips nothing.
    {
        const auto & dir = state.settings.evalCacheDir.get();
        ixe_set_eval_cache_dir(reinterpret_cast<const unsigned char *>(dir.data()), dir.size());
    }
    // How often that cache is made to prove itself. Handed over next to the
    // directory because it is meaningless without one, and unconditionally
    // because a setter called only when non-zero is a setter whose zero case
    // nobody ever tests. 0 is the default and evaluates exactly as before.
    //
    // The check exists because a cache is the one component that cannot be
    // checked by reading its output: its output is whatever it was told to
    // say. This is the only call site, so until it existed the crate's
    // `hits_disagreed` counter was structurally zero -- a clean number that
    // meant "never looked" and read as "nothing wrong" (ENG-13092).
    ixe_set_cache_verify_rate(state.settings.evalCacheVerifyRate.get());
    // The two purity settings, separately. They forbid different things, and
    // handing over `restrictEval || pureEval` as one flag -- which is what
    // this replaces -- made the evaluator refuse every host question under
    // either. `rust/nix-eval-rs/src/purity.rs` holds the per-question policy
    // and cites the cppnix line for each row; the hooks installed below are
    // why every row says "serve", since `rustCopyToStore`,
    // `rustStoreFiltered`, `rustFetch`, `rustFetchTree`, `rustFindFile` and
    // the four read hooks all go through this evaluator's own `rootFS`,
    // `checkURI` and `findFile`, so cppnix's access control applies to them
    // unchanged. With no embedder those four reads are absent and the table
    // refuses them instead, which is what the standalone probe and the
    // differential harness see.
    ixe_set_pure_eval(state.settings.pureEval ? 1 : 0);
    ixe_set_restrict_eval(state.settings.restrictEval ? 1 : 0);
    // Which names `builtins` has is not the same under every configuration:
    // an experimental feature, `allow-unsafe-native-code-during-evaluation`,
    // the `.internal` flag and the `libexpr:wasm` meson option each remove
    // one. Rather than mirror four rules on the Rust side -- where the last
    // one, being a build fact, is invisible -- hand over the answer this
    // evaluator already computed. Without it the Rust backend advertised
    // eight names cppnix hides, so `builtins ? name`, which exists so code
    // can take a working path when a builtin is absent, answered true for
    // every one of them (ENG-12717).
    {
        std::string names;
        for (auto & attr : *state.getBuiltins().attrs()) {
            if (!names.empty())
                names += ' ';
            names += state.symbols[attr.name];
        }
        setOnce(
            "the cppnix builtins name set",
            ixe_set_cpp_builtin_names(reinterpret_cast<const unsigned char *>(names.data()), names.size()));
    }
    // The path and URL literal lints live in cppnix's parser, and the Rust
    // compiler now mirrors them at its own literal sites (compile.rs,
    // ENG-12597), so the levels are forwarded rather than refused -- this
    // replaces a requireLintIgnored() that sent every evaluation under a
    // `fatal` lint back by name, including the five eval-okay corpus cases
    // that set `fatal` and then use the form the lint permits.
    //
    // Only `fatal` decides a value: it makes the program illegal, which is
    // meaning and tier 1's business. At `warn` cppnix prints a diagnostic
    // the Rust arm does not; that is warning text, tier 2 (CLAUDE.md,
    // "Parity bar"), where functional equivalence suffices -- the same line
    // this bridge drew when it refused `fatal` and passed `warn` (ENG-12569,
    // measured at nine eval-okay cases lost to refusing `warn`). The level
    // still crosses whole, so the backend knows the setting rather than this
    // bridge deciding what it may hear.
    auto lintLevel = [](const Setting<Diagnose> & setting) {
        switch (setting.get()) {
        case Diagnose::Fatal:
            return 2;
        case Diagnose::Warn:
            return 1;
        case Diagnose::Ignore:
            return 0;
        }
        return 0;
    };
    ixe_set_lint_url_literals(lintLevel(state.settings.lintUrlLiterals));
    ixe_set_lint_short_path_literals(lintLevel(state.settings.lintShortPathLiterals));
    ixe_set_lint_absolute_path_literals(lintLevel(state.settings.lintAbsolutePathLiterals));
    // The store this evaluation copies interpolated paths into. Installed per
    // call rather than once, because the state it answers out of is this
    // call's.
    // The two settings in the trace family that decide values rather than
    // output, so unlike the hooks above they have to be forwarded or the two
    // evaluators answer differently. cppnix picks `prim_trace` or
    // `prim_second` for `builtins.traceVerbose` from the first
    // (`primops.cc:5560`) -- and `prim_second` never forces the message, so
    // `traceVerbose (throw "x") 1` is `1` with it off and dead with it on.
    // The second turns `builtins.warn` into a failure (`primops.cc:1369`).
    // Both are in the Rust evaluator's memo key for the same reason.
    ixe_set_trace_verbose(state.settings.traceVerbose ? 1 : 0);
    ixe_set_abort_on_warn(state.settings.builtinsAbortOnWarn ? 1 : 0);
    // And the experimental feature that decides what `__contentAddressed =
    // true` evaluates to: the feature-is-disabled error with it off, a
    // floating-CA `.drv` with it on (`primops.cc:1632`).
    ixe_set_ca_derivations(experimentalFeatureSettings.isEnabled(Xp::CaDerivations) ? 1 : 0);
    // Same shape for `pipe-operators`, which decides at parse time whether
    // `a |> f` is the feature-is-disabled error or `f a` (lexer.l,
    // parser.y:287-295).
    ixe_set_pipe_operators(experimentalFeatureSettings.isEnabled(Xp::PipeOperators) ? 1 : 0);
    // And for `parse-toml-timestamps`, which decides whether a TOML date in
    // `builtins.fromTOML` is a `{ _type = "timestamp"; }` set or the
    // dates-are-not-supported error (primops.cc, prim_fromTOML).
    ixe_set_parse_toml_timestamps(experimentalFeatureSettings.isEnabled(Xp::ParseTomlTimestamps) ? 1 : 0);
}

const IxeHostVtable * RustEvalSetup::host() const
{
    return &hostState->vtable;
}

RustEvalSetup::~RustEvalSetup()
{
    // Read the counters back before the state goes, and hand them to the
    // census `printStatistics` reads. The evaluator does not decide whether
    // anyone looks -- it performs no IO, which is what keeps a recorded read
    // set complete -- so it accumulates and this is the one place that pulls
    // (ENG-12859).
    if (char * line = ixe_perf_snapshot()) {
        EvalPerfCensus::record(line);
        ixe_string_free(line);
    }
}

/// The token the evaluator set for its most recent refusal, or the sentinel.
///
/// `ixe_session_refusal_token` returns null when the last failure was not a
/// refusal, and static storage otherwise -- so this neither frees nor copies.
/// A null becomes `unrecorded` rather than an empty string, because an empty
/// token is a histogram row with no name and the sentinel is a row that says
/// what it is.
static std::string_view refusalTokenOf(IxeSession * session)
{
    if (!session)
        return unrecordedRefusal;
    const char * token = ixe_session_refusal_token(session);
    return token ? std::string_view(token) : unrecordedRefusal;
}

std::shared_ptr<const Pos>
rustEvalPos(EvalState & state, const std::string & source, const char * file, uint32_t line, uint32_t column)
{
    // Line 0 is the evaluator saying it has no position, which is a real
    // answer: an error raised with none of the user's source on the frame
    // stack has nowhere to point, and cppnix prints no `at ...` line for
    // those either. A fabricated 1:1 would be worse than none -- it points at
    // a line that had nothing to do with the failure, and the reader cannot
    // tell it apart from a right one.
    if (line == 0)
        return nullptr;
    Pos::Origin origin = file ? Pos::Origin(state.rootPath(std::string_view(file)))
                              : Pos::Origin(Pos::String{make_ref<std::string>(source)});
    return std::make_shared<Pos>(line, column, origin);
}

void rustEvalThrow(
    EvalState & state, int status, const std::string & message, std::string_view token, std::shared_ptr<const Pos> pos)
{
    switch (status) {
    case 1:
        // `atPos(shared_ptr<const Pos>)` and not `atPos(PosIdx)`: a `PosIdx`
        // names an entry in cppnix's own `PosTable`, and there is none to
        // point at -- nothing in this evaluation went through cppnix's
        // parser, so that table has never seen the file.
        state.error<EvalError>("%s", message).atPos(pos).debugThrow();
    case 2:
        // The evaluator refused, not the command layer, so the token is the
        // one it set rather than a constant from this file. Callers that hold
        // a session pass it; `ixe_eval_expr` has none and therefore no token
        // to give, which is what `unrecorded` is for -- a census that can see
        // how much of its population it cannot classify beats one that
        // attributes it to whatever looked closest.
        RefusalCensus::record(token, message);
        throw Error("rust-eval unimplemented: %s", message);
    case 3:
        state.error<EvalError>("rust-eval parse error: %s", message).debugThrow();
    case 5: {
        // The same class and trace note cppnix's throw primop produces, so a
        // thrown error reads (and classifies) as a throw rather than as an
        // anonymous evaluation failure. Built directly rather than through
        // EvalErrorBuilder: only the templates libexpr instantiates are
        // linkable here, and the variadic addTrace is not one of them.
        ThrownError e(state, ErrorInfo{.level = lvlError, .msg = HintFmt("%s", message), .pos = pos});
        e.addTrace(nullptr, HintFmt("while calling the '%s' builtin", "throw"));
        throw e;
    }
    case 6:
        throw AssertionError(state, ErrorInfo{.level = lvlError, .msg = HintFmt("%s", message), .pos = pos});
    case 7:
        // Only reachable if a caller forgets to handle a missing attribute
        // itself, which it should: the message is a bare name, and the
        // sentence it belongs in depends on what was being selected.
        throw Error("rust-eval: '%s' not found", message);
    default:
        // Status 4 lands here, which is the point. It used to `break` out of
        // the switch in rust-eval.cc and return normally, so a bad call into
        // nix-eval-rs printed nothing and exited 0.
        throw Error("rust-eval: invalid call into nix-eval-rs (status %d): %s", status, message);
    }
}

namespace {

/// Owns a string the C ABI handed back, so every exit path frees it.
struct IxeString
{
    char * s = nullptr;

    IxeString() = default;
    IxeString(const IxeString &) = delete;
    IxeString & operator=(const IxeString &) = delete;

    ~IxeString()
    {
        ixe_string_free(s);
    }

    std::string str() const
    {
        return s ? std::string(s) : std::string();
    }
};

/// Owns a buffer from `ixe_attrs_names`. Distinct from `IxeString` because
/// the buffer holds a NUL after every name, so it needs its length both to
/// be walked and to be freed.
struct IxeNames
{
    char * p = nullptr;
    size_t len = 0;

    IxeNames() = default;
    IxeNames(const IxeNames &) = delete;
    IxeNames & operator=(const IxeNames &) = delete;

    ~IxeNames()
    {
        ixe_names_free(p, len);
    }

    /// The names as a set, which is the shape `Suggestions::bestMatches`
    /// takes and the shape cppnix's own miss branch builds.
    StringSet set() const
    {
        StringSet names;
        for (size_t i = 0; i < len;) {
            size_t n = strnlen(p + i, len - i);
            names.emplace(p + i, n);
            i += n + 1;
        }
        return names;
    }
};

/// Owns a session and everything in its handle table.
struct RustError
{
    std::string message;
    /// Where in the user's source it happened, or null when nowhere.
    std::shared_ptr<const Pos> pos;
};

struct IxeSessionRef
{
    IxeSession * p;

    /// `setup` must outlive this session: the vtable is copied, but the
    /// context object and the answer buffers it points at belong to the
    /// setup. Every caller here keeps both on one stack frame, the setup
    /// declared first.
    ///
    /// `p` is null when the evaluator refused the host, which today means a
    /// partial set of the five path reads; the callers check it and throw.
    explicit IxeSessionRef(const RustEvalSetup & setup)
        : p(ixe_session_new(setup.host()))
    {
    }

    /// The text this session is evaluating.
    ///
    /// Kept because a position whose origin is a string has to carry the
    /// string: cppnix renders `at «string»:L:C` with the expression quoted
    /// underneath, and it reads those lines out of the origin rather than off
    /// disk. `askQuestion` sets it, which is the one place a session is given
    /// something to evaluate.
    std::string source;

    IxeSessionRef(const IxeSessionRef &) = delete;
    IxeSessionRef & operator=(const IxeSessionRef &) = delete;

    ~IxeSessionRef()
    {
        ixe_session_free(p);
    }

    /// What the last non-zero status left behind: the message, and where in
    /// the user's source it happened.
    ///
    /// One call and one value rather than a message here and a position
    /// there, because the ABI hands them over together for a reason: two
    /// accessors could be called in either order, and the order that asks for
    /// the position after taking the message gets nothing, silently.
    ///
    /// An empty message is itself a bug in the Rust side rather than a
    /// silence to paper over, so the caller gets a placeholder it can grep
    /// for.
    RustError takeError(EvalState & state) const
    {
        IxeString message;
        IxePos at = {nullptr, 0, 0};
        message.s = ixe_session_take_error(p, &at);
        auto pos = rustEvalPos(state, source, at.file, at.line, at.column);
        ixe_string_free(at.file);
        return RustError{message.s ? message.str() : "(no message)", pos};
    }

    /// Send whatever the evaluator wants to say about a damaged cache entry
    /// to stderr. Not into the returned value: the value is the expression's
    /// answer, and a cache complaint is not part of it.
    void drainWarnings() const
    {
        while (true) {
            IxeString warning;
            warning.s = ixe_session_take_warning(p);
            if (!warning.s)
                return;
            // Drained either way -- the queue must not carry over into the
            // next call -- and printed only when this arm is the one being
            // read. See `ShadowAttempt`.
            if (shadowAttempt.active)
                continue;
            std::cerr << "rust-eval: warning: " << warning.str() << "\n";
        }
    }

    [[noreturn]] void fail(EvalState & state, int status) const
    {
        auto failure = takeError(state);
        rustEvalThrow(state, status, failure.message, refusalTokenOf(p), failure.pos);
    }
};

/// Describe the evaluand's arguments in the ABI's terms.
///
/// A view over `evaluand.args`, not a copy: `IxeArgument` holds a pointer, so
/// the evaluand has to outlive the call this is passed to. Every caller here
/// keeps it on the stack for the whole session.
///
/// The bridge no longer *builds* these values. It used to, with
/// `ixe_alloc_json` and `ixe_internal_primop`, and applied them with
/// `ixe_apply` after the root came back -- which is why an evaluand with
/// arguments could not be memoised at all: the memo key knew about the
/// source and knew nothing about what the bridge had applied to it, so two
/// flakes were one key. Handing the list to `ixe_session_eval_question`
/// instead makes one list both the key and the value, and the ABI refuses the
/// three building calls while a question is in flight so that nobody can
/// reintroduce the gap. ENG-12915.
static std::vector<IxeArgument> rustArgumentViews(const RustEvaluand & evaluand)
{
    std::vector<IxeArgument> views;
    views.reserve(evaluand.args.size());
    for (auto & argument : evaluand.args)
        views.push_back(
            IxeArgument{
                .kind = argument.kind == RustArgument::Kind::Json ? IXE_ARG_JSON : IXE_ARG_INTERNAL_PRIMOP,
                .text =
                    IxeBytes{
                        .text = reinterpret_cast<const unsigned char *>(argument.text.data()),
                        .len = argument.text.size(),
                    },
            });
    return views;
}

/// The candidate attribute paths, in the ABI's terms. A view, as above.
static std::vector<IxeBytes> rustAttrPathViews(const RustEvaluand & evaluand)
{
    std::vector<IxeBytes> views;
    views.reserve(evaluand.attrPaths.size());
    for (auto & path : evaluand.attrPaths)
        views.push_back(
            IxeBytes{
                .text = reinterpret_cast<const unsigned char *>(path.data()),
                .len = path.size(),
            });
    return views;
}

/// Owns one handle. Frees on scope exit, so an exception thrown mid-walk
/// does not leak the handles the walk had opened.
struct IxeValue
{
    IxeSession * session = nullptr;
    IxeHandle handle = 0;

    IxeValue() = default;

    IxeValue(IxeValue &&) = delete;
    IxeValue(const IxeValue &) = delete;
    IxeValue & operator=(const IxeValue &) = delete;

    ~IxeValue()
    {
        if (session && handle)
            ixe_handle_free(session, handle);
    }

    void reset(IxeSession * s, IxeHandle h)
    {
        if (session && handle)
            ixe_handle_free(session, handle);
        session = s;
        handle = h;
    }

    /// Give up ownership and return the handle. For the one place a handle
    /// changes owner: an empty attribute path selects the value the source
    /// produced, so the walk's result *is* the root, and two `IxeValue`s
    /// holding it would free it twice.
    IxeHandle release()
    {
        auto released = handle;
        session = nullptr;
        handle = 0;
        return released;
    }
};

/// cppnix's attribute-path splitter, which understands quoted components
/// (`a."b.c"`). Copied rather than shared because libexpr keeps it static and
/// exporting it would widen a public header for one caller.
Strings splitAttrPath(std::string_view s)
{
    Strings res;
    std::string cur;
    auto i = s.begin();
    while (i != s.end()) {
        if (*i == '.') {
            res.push_back(cur);
            cur.clear();
        } else if (*i == '"') {
            ++i;
            while (true) {
                if (i == s.end())
                    throw ParseError("missing closing quote in selection path '%1%'", s);
                if (*i == '"')
                    break;
                cur.push_back(*i++);
            }
        } else
            cur.push_back(*i);
        ++i;
    }
    if (!cur.empty())
        res.push_back(cur);
    return res;
}

int renderMode(RustRender render)
{
    switch (render) {
    case RustRender::Json:
        return IXE_RENDER_JSON;
    case RustRender::Raw:
        return IXE_RENDER_RAW;
    case RustRender::ValuePrinter:
        return IXE_RENDER_VALUE_PRINTER;
    case RustRender::Xml:
        return IXE_RENDER_XML;
    case RustRender::Plain:
    default:
        return IXE_RENDER_PLAIN;
    }
}

} // namespace

namespace {

/// What `askQuestion` came back with, in the caller's terms.
///
/// Three outcomes, named, because two of them carry an answer and a caller
/// distinguishing them by which pointer is null would have to get a two-way
/// test right. The one it would get wrong quietly is Verify, whose failure
/// mode is skipping the check nothing else performs.
enum class Served {
    /// The cache answered. `answer` is it; do no work.
    Answer,
    /// Nothing was cached. `root` is the expression; do the work and report it.
    Evaluate,
    /// A sampled check of a cached answer. Do the work, report it, and return
    /// `answer` -- the cached one -- so the command's output does not depend
    /// on whether the sampler picked this run.
    Verify,
};

/// Ask nix-eval-rs one whole question, and find out whether it is already
/// answered.
///
/// The whole question, not just the source: which attribute path and which
/// bytes at the end of it are as much a part of what is being asked as the
/// expression is, and a memo key that leaves them out can only serve the one
/// caller whose question never varies. That is why `eval-cache-dir` wrote
/// objects for `nix eval` and `nix build` and served neither of them until
/// ENG-12830.
Served askQuestion(
    EvalState & state,
    IxeSessionRef & session,
    IxeValue & root,
    std::string & answer,
    const RustEvaluand & evaluand,
    int kind,
    int render)
{
    int mode = IXE_SERVE_EVALUATE;
    IxeHandle handle = 0;
    IxeString served;
    auto & source = evaluand.src.source;
    session.source = source;
    auto & baseDir = evaluand.src.baseDir;
    auto & file = evaluand.src.file;
    // Views into `evaluand`, which the caller holds for the whole session.
    auto args = rustArgumentViews(evaluand);
    auto attrPaths = rustAttrPathViews(evaluand);
    int rc = ixe_session_eval_question(
        session.p,
        reinterpret_cast<const unsigned char *>(source.data()),
        source.size(),
        reinterpret_cast<const unsigned char *>(baseDir.data()),
        baseDir.size(),
        // Empty means no file, which the Rust side reads as a string origin:
        // `data()` on an empty std::string is non-null, so the length is what
        // carries the distinction.
        file.empty() ? nullptr : reinterpret_cast<const unsigned char *>(file.data()),
        file.size(),
        args.empty() ? nullptr : args.data(),
        args.size(),
        kind,
        attrPaths.empty() ? nullptr : attrPaths.data(),
        attrPaths.size(),
        evaluand.indexLists ? 1 : 0,
        render,
        &mode,
        &handle,
        &served.s);
    session.drainWarnings();
    if (rc != 0)
        session.fail(state, rc);
    answer = served.str();
    if (handle)
        root.reset(session.p, handle);
    switch (mode) {
    case IXE_SERVE_ANSWER:
        return Served::Answer;
    case IXE_SERVE_VERIFY:
        return Served::Verify;
    case IXE_SERVE_EVALUATE:
        return Served::Evaluate;
    default:
        /* A mode this build does not know is the two sides disagreeing about
           the protocol, which is exactly when guessing is worst: guessing
           Answer returns an empty string as the value, and guessing Evaluate
           silently drops a verification. */
        throw Error("rust-eval: unknown serve mode %1% from the evaluation cache", mode);
    }
}

/// Tell nix-eval-rs what the walk produced, so the next process can skip it.
///
/// Unconditional at every call site, including the ones that were served: it
/// does nothing when no question is in flight, and one line that is always
/// there cannot be forgotten in the branch nobody tested.
///
/// There is deliberately no failure counterpart. Any throw between the two
/// calls leaves the question unfiled, because `~IxeSessionRef` drops the
/// scope and only this call records. That is the behaviour we want and not an
/// oversight: a failure on this path can be raised by this bridge rather than
/// by the evaluator -- a missing attribute carrying the sibling names it
/// suggests, a refusal carrying a token -- and none of those survive a round
/// trip through the (status, text) pair a memo row holds. ENG-12857.
void reportAnswer(IxeSessionRef & session, const std::string & answer)
{
    ixe_session_question_answer(session.p, 0, reinterpret_cast<const unsigned char *>(answer.data()), answer.size());
    session.drainWarnings();
}

/// Walk `attrPath` from `root`, leaving `current` on what it reached.
///
/// `false` when a component was not there, with `suggestions` filled from the
/// names at the point the walk stopped and `missing` naming the component --
/// the caller decides whether that is the end of the story or the next
/// candidate's turn. Anything else (a failure while forcing, a function met
/// partway) still throws, because a candidate that failed to *evaluate* is
/// not a candidate that was absent, and moving on would hide the failure
/// behind a later "not found".
bool walkAttrPathFrom(
    EvalState & state,
    IxeSessionRef & session,
    IxeHandle root,
    const std::string & attrPath,
    bool indexLists,
    IxeValue & current,
    Suggestions & suggestions,
    std::string & missing)
{
    current.reset(session.p, 0);
    IxeHandle here = root;

    for (auto & component : splitAttrPath(attrPath)) {
        // cppnix auto-calls a function it meets partway along a selection
        // path, using --arg/--argstr and the formals' defaults. Nothing here
        // does, and quietly selecting an attribute off the unapplied function
        // would be a different value, so say which step it was.
        if (ixe_value_type(session.p, here) == IXE_TYPE_UNFORCED) {
            int rc = ixe_force(session.p, here);
            if (rc != 0)
                session.fail(state, rc);
        }
        int type = ixe_value_type(session.p, here);
        if (type == IXE_TYPE_FUNCTION)
            refuse(
                refusalTokens::unsupported,
                "auto-calling the function reached at '%s' in selection path '%s'",
                component,
                attrPath);

        IxeHandle next = 0;
        // An all-digit component indexes a list, the same rule
        // `findAlongAttrPath` uses -- and only where cppnix would use that
        // walker. A flake is walked by `AttrCursor::findAlongAttrPath`
        // (`eval-cache.cc:514`), which only ever asks for an attribute, so
        // there `0` is an attribute name and indexing would answer where
        // cppnix reports nothing found.
        if (auto index = string2Int<unsigned int>(component); indexLists && index && type == IXE_TYPE_LIST) {
            int rc = ixe_list_at(session.p, here, *index, &next);
            if (rc == IXE_ERR_MISSING) {
                (void) session.takeError(state);
                throw Error("list index %1% in selection path '%2%' is out of range", *index, attrPath);
            }
            if (rc != 0)
                session.fail(state, rc);
        } else {
            if (component.empty())
                throw Error("empty attribute name in selection path '%1%'", attrPath);
            int rc = ixe_attrs_select(
                session.p, here, reinterpret_cast<const unsigned char *>(component.data()), component.size(), &next);
            if (rc == IXE_ERR_MISSING) {
                // Suggestions come from the names, which are readable without
                // forcing anything, so an unhelpful "not found" would be a
                // choice rather than a limitation.
                //
                // One crossing for the whole set, never one per name: this
                // used to walk an index accessor, which had to rebuild and
                // re-sort the entire name list to answer each call, and on
                // nixpkgs' 25,442-name top level that turned a message cppnix
                // produces in 2 seconds into 42. ENG-12913. cppnix's own miss
                // branch in attr-path.cc is a single pass over Bindings for
                // the same reason; this is that pass, across the ABI.
                StringSet names;
                IxeNames buf;
                if (ixe_attrs_names(session.p, here, &buf.p, &buf.len) == 0)
                    names = buf.set();
                (void) session.takeError(state);
                suggestions += Suggestions::bestMatches(names, component);
                // Which component, not just which path: cppnix's message
                // names it, and a single-candidate walk reports cppnix's
                // message verbatim.
                missing = component;
                return false;
            }
            if (rc != 0)
                session.fail(state, rc);
        }
        current.reset(session.p, next);
        here = next;
    }

    // An empty attribute path leaves `current` unset, meaning "the value the
    // source produced". The caller owns the root and hands it over there,
    // rather than this borrowing a handle somebody else will free.
    return true;
}

/// Select the first attribute path that resolves.
///
/// The candidate list is what makes `nixpkgs#hello` mean
/// `legacyPackages.<system>.hello`. It comes from
/// `InstallableFlake::getActualAttrPaths` rather than from a ladder written
/// here, and taking the first that resolves is `getCursor`'s `.at(0)`.
///
/// `current` is whatever `ixe_session_eval_question` handed back, which
/// already has the evaluand's arguments applied to it -- lazily, so nothing
/// below the root has run. The bridge used to apply them here; it cannot now,
/// because a value applied outside the question call would be outside the
/// memo key. See `rustArgumentViews`.
void selectFrom(EvalState & state, IxeSessionRef & session, IxeValue & current, const RustEvaluand & evaluand)
{
    IxeValue root;
    root.reset(session.p, current.release());

    Suggestions suggestions;
    std::string missing;
    for (auto & attrPath : evaluand.attrPaths) {
        if (walkAttrPathFrom(
                state, session, root.handle, attrPath, evaluand.indexLists, current, suggestions, missing)) {
            // An empty attribute path selects the root itself.
            if (!current.handle)
                current.reset(session.p, root.release());
            /* Force the selected value before anything reads it, so a failure
               that happens later is known to be below the root. For the
               printer that is the whole test for "cppnix would have printed
               «error: ...» here and carried on": the root is already in weak
               head normal form, so nothing the renderer does to the root
               itself can fail.

               After the candidate is chosen and not during the search: a
               candidate that exists and throws is a failure, not an absence,
               and cppnix does not move on from it either -- `getCursors`
               collects the paths that exist and `getCursor` forces one. */
            if (int rc = ixe_force(session.p, current.handle); rc != 0)
                session.fail(state, rc);
            return;
        }
    }

    // Nothing resolved. Two different messages, because cppnix has two: a
    // flake reports every path it tried, while `--expr`/`--file` reports the
    // one it had.
    if (evaluand.flakeRef) {
        std::string tried;
        for (const auto & [n, path] : enumerate(evaluand.attrPaths)) {
            if (n > 0)
                tried += n + 1 == evaluand.attrPaths.size() ? " or " : ", ";
            tried += '\'';
            tried += path;
            tried += '\'';
        }
        throw Error(suggestions, "flake '%s' does not provide attribute %s", *evaluand.flakeRef, tried);
    }
    throw AttrPathNotFound(
        suggestions,
        "attribute '%1%' in selection path '%2%' not found",
        missing,
        evaluand.attrPaths.empty() ? "" : evaluand.attrPaths.front());
}

} // namespace

std::string
rustEvalRender(EvalState & state, const RustEvaluand & evaluand, RustRender render, bool nestedFailureIsUnimplemented)
{
    RustEvalSetup setup(state);

    IxeSessionRef session(setup);
    if (!session.p) {
        /* The evaluator refused the host it was handed. The one way that
           happens today is a partial set of the five path reads, which
           would leave this evaluator reading outside the process's access
           control; the reason comes back through the setting-conflict slot
           rather than being guessed at here. */
        IxeString why;
        why.s = ixe_take_setting_conflict();
        throw Error(
            "rust-eval: could not create an evaluation session: %s",
            why.s ? why.str() : "the evaluator gave no reason");
    }

    /* One path, whether or not the evaluand has arguments. It used to be two,
       because a flake's arguments were applied by this bridge after the fact
       and appeared in no memo key, so `mayBeMemoised` sent every flake down a
       branch that could not be served. The arguments now cross on this call
       and are keyed on, which is ENG-12915 and is what gives `nix eval
       <flake>#attr` warm starts. */
    IxeValue current;
    std::string served;
    auto mode = askQuestion(state, session, current, served, evaluand, IXE_QUESTION_SELECT, renderMode(render));
    if (mode == Served::Answer)
        return served;

    selectFrom(state, session, current, evaluand);

    IxeString out;
    int rc = ixe_render(session.p, current.handle, renderMode(render), &out.s);
    session.drainWarnings();
    if (rc != 0) {
        auto failure = session.takeError(state);
        if (nestedFailureIsUnimplemented && rc != 2 /* already unimplemented */)
            refuse(
                refusalTokens::unsupported,
                "printing a value that fails below the top level "
                "(cppnix prints «error: %s» in place and carries on)",
                failure.message);
        rustEvalThrow(state, rc, failure.message, refusalTokenOf(session.p), failure.pos);
    }
    auto answer = out.str();
    // A no-op when no question is in flight, which is the whole of the
    // arguments case; see `reportAnswer`.
    reportAnswer(session, answer);
    return mode == Served::Verify ? served : answer;
}

std::string rustEvalSelect(
    EvalState & state,
    const std::string & source,
    const std::string & baseDir,
    const std::string & file,
    const std::string & attrPath,
    RustRender render,
    bool nestedFailureIsUnimplemented)
{
    return rustEvalRender(
        state,
        RustEvaluand{
            .src = RustSource{.source = source, .baseDir = baseDir, .file = file},
            .args = {},
            .attrPaths = {attrPath},
        },
        render,
        nestedFailureIsUnimplemented);
}

/// The exception class, most derived first.
///
/// Ordered because `ThrownError` is an `AssertionError` is an `EvalError`, so
/// a ladder that tested the base first would call every failure `eval`. The
/// names are the class names rather than invented tokens: the whole point is
/// that both arms are classified by the same rule, and the rule is the C++
/// type the throw produced.
std::string shadowErrorClass(const std::exception & e)
{
    if (dynamic_cast<const ThrownError *>(&e))
        return "throw";
    if (dynamic_cast<const AssertionError *>(&e))
        return "assert";
    if (dynamic_cast<const Abort *>(&e))
        return "abort";
    if (dynamic_cast<const TypeError *>(&e))
        return "type";
    if (dynamic_cast<const UndefinedVarError *>(&e))
        return "undefined-variable";
    if (dynamic_cast<const MissingArgumentError *>(&e))
        return "missing-argument";
    if (dynamic_cast<const InfiniteRecursionError *>(&e))
        return "infinite-recursion";
    if (dynamic_cast<const StackOverflowError *>(&e))
        return "stack-overflow";
    if (dynamic_cast<const ParseError *>(&e))
        return "parse";
    if (dynamic_cast<const EvalError *>(&e))
        return "eval";
    if (dynamic_cast<const Error *>(&e))
        return "error";
    /* Not a nix error at all: an allocation failure, a bad_cast out of the
       bridge, anything the Rust arm's C++ side got wrong. Its own name
       because "the backend broke" and "the backend disagrees" are different
       findings. */
    return "non-nix-exception";
}

namespace {

/// The message of a caught exception, with nix's own accessor where there is
/// one.
///
/// `e.what()` on a `nix::Error` is the *formatted* error -- "error: " prefix,
/// ANSI colour and all -- while `e.message()` is the text. Mixing the two is
/// not a small cosmetic slip: the first run of this comparator took
/// `e.message()` from the C++ arm and `e.what()` from the Rust arm and
/// reported every single failing case as a divergence, because one side
/// carried a prefix the other did not.
std::string shadowMessageOf(const std::exception & e)
{
    if (auto * error = dynamic_cast<const Error *>(&e))
        return error->message();
    return e.what();
}

/// The same text with ANSI escapes removed and newlines flattened.
///
/// nix colours its messages whenever the stream looks like a terminal, so a
/// comparison of raw bytes compares two colourings as much as two messages --
/// the shape CLAUDE.md warns about, where a pattern is matched against a
/// string the writer never emitted. Flattened as well, because the report is
/// one line per divergence and an embedded newline would silently split it
/// into two records for anything parsing the journal.
std::string shadowPlain(const std::string & text)
{
    std::string out;
    out.reserve(text.size());
    for (size_t i = 0; i < text.size(); i++) {
        if (text[i] == '\x1b') {
            // CSI ... final byte in @-~. Anything else is left alone rather
            // than guessed at.
            size_t j = i + 1;
            if (j < text.size() && text[j] == '[') {
                j++;
                while (j < text.size() && !(text[j] >= '@' && text[j] <= '~'))
                    j++;
                i = j;
                continue;
            }
        }
        out += text[i] == '\n' ? ' ' : text[i];
    }
    return out;
}

/// A value or message, cut to something a log line can carry.
///
/// The length is kept even when the text is not, because "these two differ"
/// is a much weaker statement than "these two differ and one of them is
/// 400 kB", and a truncated pair that happens to agree in its first 200 bytes
/// would otherwise read as an identical pair.
std::string shadowTruncate(const std::string & text)
{
    constexpr size_t limit = 200;
    if (text.size() <= limit)
        return text;
    /* Back off the cut to a character boundary. A UTF-8 continuation byte is
       0b10xxxxxx, so while the byte AT the cut is one, the cut is inside a
       character and the byte before it belongs to the same character.
       At most three steps: no UTF-8 sequence is longer than four bytes.

       This was a plain `substr(0, 200)` and the bug was not that the output
       looked wrong. Invalid UTF-8 here reaches `NIX_SHOW_STATS`, whose JSON
       writer refuses the whole document, so ONE damaged detail wrote the
       entire process census -- attempts, verdicts, tokens, every other
       divergence -- as zero bytes. A nixpkgs sweep lost 7 of 2638 attributes
       that way, and the only thing that noticed was a cross-check of stats
       files read against attributes attempted. ENG-12874.

       The writer no longer loses a document to one bad string either
       (`EvalState::printStatistics`), and that guard is the one that matters:
       this function is only the trigger that found it, and any other source
       of odd bytes in a message would have done. */
    size_t cut = limit;
    while (cut > 0 && (static_cast<unsigned char>(text[cut]) & 0xC0) == 0x80)
        --cut;
    /* The byte count is the ORIGINAL size, not the cut one: "these two differ
       and one of them is 400 kB" is the fact worth keeping. */
    return text.substr(0, cut) + "…(" + std::to_string(text.size()) + " bytes)";
}

/// Where the divergence is, as precisely as this backend can say.
///
/// File and attribute path, which is all there is: the Rust arm carries no
/// source positions at all (ENG-12714), so there is no line and column to
/// report and pretending otherwise would be worse than the honest coarse
/// answer. `<expr>` rather than an empty field for `--expr`, so a reader
/// never has to wonder whether the field failed to fill in.
std::string shadowOrigin(const ShadowSubject & subject)
{
    /* An installable says what the user typed, which is the only spelling
       that helps: a flake's evaluand names `call-flake.nix` as its source and
       carries a candidate list of expanded attribute paths, so reconstructing
       `nixpkgs#hello` from it is impossible and guessing at it would print a
       path the user never wrote. */
    if (!subject.what.empty())
        return subject.what;
    auto where = subject.evaluand.src.file.empty() ? std::string("<expr>") : subject.evaluand.src.file;
    auto attrPath = subject.evaluand.attrPaths.empty() ? std::string() : subject.evaluand.attrPaths.front();
    return attrPath.empty() ? where : where + "#" + attrPath;
}

/// The part of the origin that is the same on every machine: the file's own
/// name and the attribute path, without the directories above it.
///
/// The full origin is what a human needs in order to go and look, and it is
/// still reported. It cannot go in the id, because it is absolute -- and an
/// absolute path is exactly the component that differs between two checkouts
/// of the same tree. See `shadowId`.
std::string shadowIdOrigin(const ShadowSubject & subject)
{
    if (!subject.what.empty()) {
        /* `what` is `<flake ref>#<fragment>`, and a *local* flake ref carries
           the checkout's absolute directory -- the one component that differs
           between two checkouts of the same tree, and exactly what the file
           case strips. A remote ref (`github:NixOS/nixpkgs`, `nixpkgs`) has
           no directory to strip and is already the same everywhere, so it is
           kept whole: cutting at its last slash would turn every flake on
           GitHub into its bare repository name and group two projects with
           the same repo name into one row. */
        auto hash = subject.what.find('#');
        auto ref = subject.what.substr(0, hash);
        auto rest = hash == std::string::npos ? std::string() : subject.what.substr(hash);
        auto local = ref.starts_with('/') || ref.starts_with("path:") || ref.starts_with("git+file:");
        auto slash = ref.rfind('/');
        if (local && slash != std::string::npos)
            ref = ref.substr(slash + 1);
        return ref + rest;
    }
    auto where = subject.evaluand.src.file.empty()
                     ? std::string("<expr>")
                     : std::filesystem::path(subject.evaluand.src.file).filename().string();
    auto attrPath = subject.evaluand.attrPaths.empty() ? std::string() : subject.evaluand.attrPaths.front();
    return attrPath.empty() ? where : where + "#" + attrPath;
}

/// The bytes a `Derivation` question is compared on.
///
/// `<drvPath> <output>,<output>` per derivation, one per line, sorted. Not a
/// render mode and deliberately not the value's printed form: what `nix
/// build` gets out of an evaluation is a store path and a set of output
/// names, and those are Tier 1 -- byte-identical or the two backends would
/// build different things. Printing the derivation attrset instead would
/// compare a large document full of Tier 2 presentation and bury the one
/// field that matters.
///
/// Sorted because neither arm promises an order and a difference in it would
/// be a divergence about nothing. Today both produce exactly one line, so the
/// sort is insurance rather than load-bearing.
std::string shadowDerivationLines(Store & store, const std::vector<std::pair<StorePath, StringSet>> & found)
{
    std::vector<std::string> lines;
    for (auto & [drvPath, outputs] : found) {
        auto line = store.printStorePath(drvPath) + " ";
        bool first = true;
        for (auto & name : outputs) {
            if (!first)
                line += ",";
            line += name;
            first = false;
        }
        lines.push_back(std::move(line));
    }
    std::sort(lines.begin(), lines.end());
    return concatStringsSep("\n", lines);
}

/// A name for this divergence that is the same on every machine that hits it.
///
/// Over the kind, the portable part of the origin, and both results, so two
/// runs of the same broken expression group into one row and two different
/// expressions failing the same way do not.
///
/// The directories are deliberately not in here, and that is a correction
/// rather than a preference. This id previously hashed the *absolute* path,
/// which made the "same on every machine" claim in its own doc comment false:
/// the same divergence in the lang corpus produced `bc45769e3203` from a
/// macOS worktree and `03c08a51a0bb` from a Linux checkout of the identical
/// revision. A fleet query grouping by id would have reported one finding as
/// one row per host, which is precisely the failure the stable-token
/// vocabulary exists to prevent, reproduced one layer up.
///
/// Truncated results rather than whole ones, for the same reason: the id then
/// survives a value whose tail is a store hash that moves between machines.
std::string
shadowId(std::string_view kind, const std::string & origin, const std::string & left, const std::string & right)
{
    auto material = std::string(kind) + "\n" + origin + "\n" + shadowTruncate(left) + "\n" + shadowTruncate(right);
    return hashString(HashAlgorithm::SHA256, material).to_string(HashFormat::Base16, false).substr(0, 12);
}

/// Whether two renderings of the same value are the same answer.
///
/// Bytes, except for `--json`, where they are the same document. The Rust arm
/// returns compact JSON and `nix eval --json` re-dumps through nlohmann, so a
/// byte comparison there would be comparing two serializers and reporting
/// their disagreements about spacing as value divergences. Both sides go
/// through one parser instead.
///
/// A rust text that does not parse is *not* quietly treated as equal: the
/// fallback is the byte comparison, which such a text loses.
bool shadowSameRendering(RustRender render, const std::string & left, const std::string & right)
{
    if (render != RustRender::Json)
        return left == right;
    try {
        return nlohmann::json::parse(left) == nlohmann::json::parse(right);
    } catch (const nlohmann::json::exception &) {
        return left == right;
    }
}

} // namespace

void rustEvalShadow(EvalState & state, const ShadowSubject & subject, const ShadowCppOutcome & cpp)
{
    if (shadowAttempt.active) {
        ShadowCensus::skipped(ShadowSkip::Reentrant);
        return;
    }
    auto budgetMicros = static_cast<uint64_t>(state.settings.evalShadowBudget.get()) * 1000000;
    if (ShadowCensus::budgetExhausted(budgetMicros)) {
        ShadowCensus::skipped(ShadowSkip::Budget);
        return;
    }

    /* Everything from here is wrapped, including the reporting. A shadow that
       threw would turn a served command into a failed one, which is the one
       thing this must never do -- and the outer catch covers the comparison
       and the report as well as the evaluation, because a bug in the reporter
       would otherwise take down exactly the runs that found something. */
    try {
        shadowAttempt.active = true;
        shadowAttempt.tripped = false;
        /* What is left of the budget, as a wall-clock deadline for this one
           attempt. Zero budget means no limit and therefore no deadline; the
           interrupt hook then does nothing it did not do before.

           This is the half of `eval-shadow-budget` that used to be missing.
           Checking the budget only on the way in bounds a command that
           evaluates many small expressions and bounds nothing at all for a
           command that evaluates one very large one -- and wiring flakes in
           makes the second kind the common case, since `nix build
           .#darwinConfigurations.<host>.system` is a single attempt whose
           Rust arm may take minutes or never finish. */
        shadowAttempt.deadline =
            budgetMicros == 0
                ? std::optional<std::chrono::steady_clock::time_point>()
                : std::chrono::steady_clock::now() + std::chrono::microseconds(budgetMicros - ShadowCensus::micros());
        Finally leave([&]() {
            shadowAttempt.active = false;
            shadowAttempt.deadline.reset();
        });

        /* Before the call, never after. An arm that dies mid-evaluation then
           leaves attempts ahead of verdicts, and `unaccounted` in the stats
           block says so. Incrementing on return would make that same death
           indistinguishable from an evaluation that never happened. */
        ShadowCensus::attempt();

        /* A refusal arrives as a plain `Error` whose only marker is a phrase
           in its message, so the verdict is read from the census instead: it
           is recorded at the moment of refusal, and the token survives a
           reworded message, which a phrase match would not. */
        auto refusalsBefore = RefusalCensus::total();
        auto start = std::chrono::steady_clock::now();

        bool rustOk = false;
        std::string rustText;
        std::string rustClass;
        std::string rustMessage;
        try {
            /* The same two entry points the served `rust` backend uses, so a
               divergence is between the evaluators and never between this
               harness and the command it is shadowing. */
            if (subject.question == ShadowQuestion::Derivation) {
                std::vector<std::pair<StorePath, StringSet>> found;
                for (auto & drv : rustEvalDerivations(state, subject.evaluand))
                    found.emplace_back(drv.drvPath, drv.outputs);
                rustText = shadowDerivationLines(*state.store, found);
            } else {
                rustText =
                    rustEvalRender(state, subject.evaluand, subject.render, subject.nestedFailureIsUnimplemented);
            }
            rustOk = true;
        } catch (std::exception & e) {
            rustClass = shadowErrorClass(e);
            rustMessage = shadowPlain(shadowMessageOf(e));
        }

        auto micros =
            std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now() - start).count();
        ShadowCensus::spent(static_cast<uint64_t>(micros));

        if (!rustOk && shadowAttempt.tripped) {
            /* The deadline stopped it, so there is no comparison to make and
               nothing here is about the evaluator. Recorded as its own
               verdict rather than as a divergence or a skip: it is an attempt
               that reached no conclusion, and the census holds `attempts`
               equal to the sum of the verdicts. */
            ShadowCensus::record(ShadowVerdict::TimedOut);
            return;
        }

        if (!rustOk && RefusalCensus::total() > refusalsBefore) {
            /* Refused. Recorded and not reported as a divergence: a construct
               the backend has not been written yet is a known gap with a
               token that already names it, and folding those into the
               divergence histogram would bury the disagreements -- which are
               the rare, interesting rows -- under the gaps. */
            ShadowCensus::record(ShadowVerdict::Refused, RefusalCensus::lastToken());
            return;
        }

        auto origin = shadowOrigin(subject);
        // Two spellings on purpose: the full one for a human to go and look
        // at, the portable one for the id, which has to be the same from two
        // checkouts of the same tree.
        auto idOrigin = shadowIdOrigin(subject);
        auto report = [&](std::string_view kind, const std::string & raw_left, const std::string & raw_right) {
            auto left = shadowPlain(raw_left);
            auto right = shadowPlain(raw_right);
            auto detail = "cpp=" + shadowTruncate(left) + " rust=" + shadowTruncate(right);
            ShadowCensus::diverged(kind, shadowId(kind, idOrigin, left, right), origin, detail);
        };

        if (cpp.ok && rustOk) {
            if (shadowSameRendering(subject.render, cpp.text, rustText)) {
                ShadowCensus::record(ShadowVerdict::Agreed);
            } else {
                report(shadowKinds::valueMismatch, cpp.text, rustText);
                ShadowCensus::record(ShadowVerdict::Mismatched);
            }
            return;
        }

        if (cpp.ok && !rustOk) {
            report(
                rustClass == "non-nix-exception" ? shadowKinds::rustCrashed : shadowKinds::rustFailed,
                cpp.text,
                rustClass + ": " + rustMessage);
            ShadowCensus::record(rustClass == "non-nix-exception" ? ShadowVerdict::Crashed : ShadowVerdict::Mismatched);
            return;
        }

        if (!cpp.ok && rustOk) {
            /* The rarer direction and the more alarming one: the Rust arm
               answered a program cppnix rejects, so somewhere it is more
               permissive than the language. */
            report(shadowKinds::cppFailed, cpp.errorClass + ": " + cpp.errorMessage, rustText);
            ShadowCensus::record(ShadowVerdict::Mismatched);
            return;
        }

        /* Both failed. The class is the bar; the wording is tier 2, reported
           under its own kind so somebody can read it, and counted apart from
           the mismatches so it cannot move the number the default flip is
           decided on. */
        if (cpp.errorClass != rustClass) {
            /* Same words, different class, is the bridge's status mapping
               being coarser than cppnix's exception hierarchy rather than
               the two evaluators disagreeing, so it gets its own row and its
               own verdict. Both are still counted; they are just not counted
               as the same thing. */
            auto sameWords = shadowPlain(cpp.errorMessage) == rustMessage;
            report(
                sameWords ? shadowKinds::errorClassLost : shadowKinds::errorClassMismatch,
                cpp.errorClass + ": " + cpp.errorMessage,
                rustClass + ": " + rustMessage);
            ShadowCensus::record(sameWords ? ShadowVerdict::AgreedFailureTextDiffers : ShadowVerdict::Mismatched);
        } else if (shadowPlain(cpp.errorMessage) != rustMessage) {
            report(shadowKinds::errorTextMismatch, shadowPlain(cpp.errorMessage), rustMessage);
            ShadowCensus::record(ShadowVerdict::AgreedFailureTextDiffers);
        } else {
            ShadowCensus::record(ShadowVerdict::AgreedFailure);
        }
    } catch (std::exception & e) {
        /* The shadow machinery itself failed. Counted as a crash, because
           from the census's point of view that is what it is -- an attempt
           that produced no comparison -- and said out loud, because a
           reporting path that fails silently is how a divergence census
           reports zero for the wrong reason. */
        ShadowCensus::record(ShadowVerdict::Crashed);
        std::cerr << "<4>rust-eval shadow: the comparison itself failed: " << e.what() << "\n";
        std::cerr.flush();
    } catch (...) {
        ShadowCensus::record(ShadowVerdict::Crashed);
        std::cerr << "<4>rust-eval shadow: the comparison itself failed with a non-exception throw\n";
        std::cerr.flush();
    }
}

/// Hand the refusal vocabulary to the census, once, at load.
///
/// The census lives in libexpr and the vocabulary is the evaluator's, defined
/// in `rust/nix-eval-rs/src/refusal.rs` and enumerable over the C ABI that
/// only this library links. Registering it rather than restating it is what
/// keeps there being one list: a second copy in libexpr would drift the first
/// time either side gained a token, and the histogram's denominator would
/// quietly stop covering it.
static const struct RegisterRefusalVocabulary
{
    RegisterRefusalVocabulary()
    {
        std::vector<std::string> tokens;
        auto count = ixe_refusal_token_count();
        tokens.reserve(count);
        for (size_t i = 0; i < count; i++)
            if (const char * name = ixe_refusal_token_at(i))
                tokens.emplace_back(name);
        RefusalCensus::setVocabulary(std::move(tokens));
    }
} registerRefusalVocabulary;

namespace {

/// Select `name` off an attribute set handle, or report that it is absent.
///
/// Absence is a normal answer here, not a failure: `outputs`, `meta` and
/// `outputSpecified` are all optional on a derivation and each has its own
/// default. Anything other than absence is the session's failure to raise.
bool selectOptional(
    EvalState & state, IxeSessionRef & session, IxeHandle attrs, const std::string & name, IxeValue & out)
{
    IxeHandle next = 0;
    int rc =
        ixe_attrs_select(session.p, attrs, reinterpret_cast<const unsigned char *>(name.data()), name.size(), &next);
    if (rc == IXE_ERR_MISSING) {
        (void) session.takeError(state);
        return false;
    }
    if (rc != 0)
        session.fail(state, rc);
    out.reset(session.p, next);
    return true;
}

/// Force a handle and read it as a string, refusing anything else by name.
///
/// `where` names the attribute for both the refusal and the census, so a
/// derivation with, say, a list where cppnix wants a string is one histogram
/// row that says which attribute rather than a shrug.
std::string forceString(EvalState & state, IxeSessionRef & session, IxeValue & value, const std::string & where)
{
    if (int rc = ixe_force(session.p, value.handle); rc != 0)
        session.fail(state, rc);
    if (ixe_value_type(session.p, value.handle) != IXE_TYPE_STRING)
        refuse(refusalTokens::notADerivation, "%s is not a string", where);
    IxeString out;
    if (int rc = ixe_render(session.p, value.handle, IXE_RENDER_RAW, &out.s); rc != 0)
        session.fail(state, rc);
    return out.str();
}

} // namespace

namespace {

/// A derivation as a memo row's answer: the drvPath, then one output name per
/// line.
///
/// A format of its own rather than a reuse of an existing renderer. What `nix
/// build` wants out of an evaluation is not printable bytes -- it is a store
/// path and a set of output names -- so there is no render mode whose output
/// would do. Newline-separated is unambiguous because neither a store path
/// nor an output name can contain one.
std::string encodeDerivation(EvalState & state, const RustDerivation & found)
{
    std::string out = state.store->printStorePath(found.drvPath);
    for (auto & name : found.outputs)
        out += "\n" + name;
    return out;
}

/// The inverse, with the checks the fresh path applies applied again.
///
/// `requireDerivation` a second time is not belt and braces. A served answer
/// is bytes off disk that nothing in this process computed, and a build
/// pointed at a path that is not a derivation is precisely the failure the
/// memo-hit ratchet exists to prevent.
RustDerivation decodeDerivation(EvalState & state, const std::string & encoded)
{
    auto lines = tokenizeString<std::vector<std::string>>(encoded, "\n");
    if (lines.empty())
        throw Error("rust-eval: the evaluation cache holds a derivation answer with no drvPath");
    auto drvPath = state.store->parseStorePath(lines.front());
    drvPath.requireDerivation();
    StringSet outputs(lines.begin() + 1, lines.end());
    /* cppnix never produces an empty output set -- `out` is the default and
       `meta.outputsToInstall` reducing to nothing is refused on the fresh
       path -- so an empty one here is a damaged row, and a build of no
       outputs is the shape nobody notices. */
    if (outputs.empty())
        throw Error("rust-eval: the evaluation cache holds a derivation answer with no outputs");
    return RustDerivation{.drvPath = std::move(drvPath), .outputs = std::move(outputs)};
}

} // namespace

/// The `overrides` argument of `call-flake.nix`, as JSON.
///
/// cppnix's `callFlake` (`flake.cc:1075`) builds this as a `Value`; this
/// builds the same thing as a document `ixe_alloc_json` decodes, because a
/// `Value` cannot cross the ABI and a lock file can.
///
/// Two things are deliberate and both were learned from `rustFetchTree`
/// above. The set is serialised attribute by attribute, never as a unit,
/// because `printValueAsJSON` collapses any attrset carrying an `outPath` to
/// that string alone (`value-to-json.cc:100`, the derivation shorthand) and a
/// `sourceInfo` has one. And `outPath` is then overwritten with the store-path
/// escape, because JSON cannot carry string context and a flake source path
/// without its own `Opaque` element is a dependency that has quietly
/// vanished -- every derivation built from `self` would lose an input.
static nlohmann::json flakeOverridesJSON(EvalState & state, const flake::LockedFlake & lockedFlake)
{
    auto [lockFileStr, keyMap] = lockedFlake.lockFile.to_string();
    (void) lockFileStr;

    auto overrides = nlohmann::json::object();
    for (auto & [node, sourcePath] : lockedFlake.nodePaths) {
        auto lockedNode = node.dynamic_pointer_cast<const flake::LockedNode>();
        auto [storePath, subdir] = state.store->toStorePath(sourcePath.path.abs());

        Value vSourceInfo;
        emitTreeAttrs(
            state,
            storePath,
            lockedNode ? lockedNode->lockedRef.input : lockedFlake.flake.lockedRef.input,
            vSourceInfo,
            false,
            !lockedNode && lockedFlake.flake.forceDirty);

        state.forceAttrs(vSourceInfo, noPos, "while serialising a flake's source info");
        auto sourceInfo = nlohmann::json::object();
        for (auto & a : *vSourceInfo.attrs()) {
            NixStringContext context;
            sourceInfo[std::string(state.symbols[a.name])] =
                printValueAsJSON(state, true, *a.value, noPos, context, false);
        }
        sourceInfo["outPath"] = nlohmann::json::object({{"__storePath", state.store->printStorePath(storePath)}});

        auto key = keyMap.find(node);
        // cppnix asserts here. A throw rather than an assert because an
        // assert is compiled out of a release build, and a node with no key
        // would silently drop an override -- which is a flake input resolving
        // to the wrong tree, not a crash.
        if (key == keyMap.end())
            throw Error("rust-eval: a locked flake node has no key in the lock file");
        overrides[key->second] = nlohmann::json::object({
            {"sourceInfo", sourceInfo},
            {"dir", CanonPath(subdir).rel()},
        });
    }
    /* How much of the lock this document covers, which decides which half of
       `call-flake.nix` runs and is otherwise unobservable from outside.
       `computeLocks` fills `nodePaths` only for nodes it fetches, so a lock
       being created covers every node -- `hasOverride` is true everywhere and
       `fetchTreeFinal` is unreachable -- while an up-to-date lock keeps
       children lazily, leaves them out, and sends them through
       `fetchTreeFinal` instead.

       Emitted because a gate cannot otherwise tell a run that exercised the
       tree fetcher from one that did not, and the two look identical in every
       value they produce. `flake-inputs-parity.sh` reads this line and refuses
       a run in which no node reached the fetcher. */
    debug("rust-eval: flake overrides cover %d of %d lock node(s)", overrides.size(), keyMap.size());
    return overrides;
}

RustEvaluand rustEvaluandOf(
    SourceExprCommand & cmd, ref<EvalState> state, const std::optional<RustSource> & source, std::string_view prefix)
{
    // "." is how a command with no positional argument spells "the whole
    // value", the same rewrite `InstallableAttrPath::parse` does.
    auto attrPath = prefix == "." ? std::string() : std::string(prefix);

    if (source) {
        rustRequireNoAutoArgs(cmd, *state);
        return RustEvaluand{.src = *source, .args = {}, .attrPaths = {attrPath}};
    }

    // cppnix tries a store path first whenever the argument contains a slash
    // (`installables.cc`), and falls through to a flake reference when that
    // does not parse. Same order here, so the two backends disagree about
    // nothing: what changes is that a store path is refused rather than
    // served.
    if (prefix.find('/') != std::string_view::npos) {
        bool isStorePath = false;
        try {
            (void) InstallableDerivedPath::parse(state->store, prefix, ExtendedOutputsSpec::Default{});
            isStorePath = true;
        } catch (BadStorePath &) {
        } catch (Error &) {
        }
        if (isStorePath)
            refuse(
                refusalTokens::installable,
                "the store-path installable '%s' (this backend evaluates a flake, an '--expr' "
                "or a '--file'; a store path names something already built)",
                prefix);
    }

    auto [flakeRef, fragment] =
        parseFlakeRefWithFragment(fetchSettings, std::string{prefix}, absPath(cmd.getCommandBaseDir()));

    /* The installable cppnix would have built, used for the two rules that
       decide *what* is selected: the candidate attribute paths, and the lock.
       Constructing it evaluates nothing -- `getCursors` does, and is not
       called -- so this is the rules and not the C++ evaluator running. A
       second copy of `getActualAttrPaths`'s prefix ladder here is how the two
       backends would come to build different packages for the same command
       line. It also raises cppnix's own UsageError for `--arg` with a flake,
       which is why `rustRequireNoAutoArgs` is on the other branch only. */
    InstallableFlake installable(
        &cmd,
        state,
        std::move(flakeRef),
        fragment,
        ExtendedOutputsSpec::Default{},
        cmd.getDefaultFlakeAttrPaths(),
        cmd.getDefaultFlakeAttrPathPrefixes(),
        cmd.lockFlags);

    /* The one thing this cannot serve, for the reason `rustFetchTree` gives
       and in the same words: `emitTreeAttrs` answers with a recording thunk
       per metadata attribute when the tracker is on, and the overrides
       document below forces every one of them. Serialising them would record
       reads the flake never made and hand the evaluator plain values that can
       never record the ones it does. */
    if (state->readSetTracker)
        refuse(
            refusalTokens::unsupported,
            "a flake installable while the read-set tracker is on (the overrides this hands "
            "over are cppnix's emitTreeAttrs sets, which are per-attribute recording thunks "
            "under the tracker, and serialising them would both record reads nobody made and "
            "lose the ones the flake does make)");

    /* The one place the C++ evaluator serves under `eval-backend = rust`.
       `lockFlake` evaluates `flake.nix` to read its `inputs`, walks the input
       graph, consults the registry and writes `flake.lock`; all of that is IO
       and policy cppnix owns, and none of it is a value the user asked for.
       What crosses out of this scope is a lock file. The flake's `outputs`
       are evaluated below, by the Rust backend, out of `call-flake.nix`.

       Stated because it bounds the parity claim: what is being compared
       downstream is the two evaluators' reading of `outputs`, not of
       `inputs`, which cppnix reads on both arms. */
    std::shared_ptr<flake::LockedFlake> lockedFlake;
    {
        EvalState::LockingFlake locking(*state);
        lockedFlake = installable.getLockedFlake().get_ptr();
    }
    auto [lockFileStr, keyMap] = lockedFlake->lockFile.to_string();
    (void) keyMap;

    Strings attrPaths;
    for (auto & candidate : installable.getActualAttrPaths())
        attrPaths.push_back(candidate);

    return RustEvaluand{
        .src =
            RustSource{
                .source = std::string(flake::callFlakeSource()),
                // Nothing in `call-flake.nix` names a relative path, and
                // cppnix gives it a source accessor with no directory either.
                .baseDir = "/",
                // Empty, so `__curPos` answers `null`: cppnix's origin for
                // this file is «flakes-internal», which is not a filesystem
                // path and which naming here would make up.
                .file = "",
            },
        .args =
            {
                RustArgument{.kind = RustArgument::Kind::Json, .text = nlohmann::json(lockFileStr).dump()},
                RustArgument{.kind = RustArgument::Kind::Json, .text = flakeOverridesJSON(*state, *lockedFlake).dump()},
                RustArgument{.kind = RustArgument::Kind::InternalPrimop, .text = "fetchFinalTree"},
            },
        .attrPaths = attrPaths,
        .indexLists = false,
        .flakeRef = installable.flakeRef.to_string(),
    };
}

std::optional<RustEvaluand> shadowEvaluandOfInstallable(EvalState & state, Installable & installable)
{
    try {
        /* A flake, or nothing this can describe. `--expr`/`--file` reach the
           shadow arm through `rustReadSource` on the command, because the
           `InstallableAttrPath` they become keeps its source and its value
           private and there is nothing to recover them from here. */
        auto * flake = dynamic_cast<InstallableFlake *>(&installable);
        if (!flake) {
            ShadowCensus::skipped(ShadowSkip::NonValueInstallable);
            return std::nullopt;
        }

        // `foo^out` selects derivation outputs, which needs derivations; the
        // served backend refuses it by name and here it is simply not
        // compared.
        if (!std::get_if<ExtendedOutputsSpec::Default>(&flake->extendedOutputsSpec.raw)) {
            ShadowCensus::skipped(ShadowSkip::UnservableShape);
            return std::nullopt;
        }

        /* The reason `rustEvaluandOf` refuses the same case, in the same
           words: `emitTreeAttrs` answers with a recording thunk per metadata
           attribute when the tracker is on, and the overrides document below
           forces every one of them. Serialising them would record reads the
           flake never made and hand the evaluator plain values that can never
           record the ones it does. */
        if (state.readSetTracker) {
            ShadowCensus::skipped(ShadowSkip::FlakeUnservable);
            return std::nullopt;
        }

        /* The lock the served arm already computed, read straight off the
           installable, and **never** locked here. This is the whole reason
           this function exists rather than a call to `rustEvaluandOf`: that
           one builds a second `InstallableFlake` and locks it, and locking
           walks the input graph, consults the registry and writes
           `flake.lock`. Under shadow the C++ arm has already served the user
           by this point, so a second lock would be fetching and writing on
           the user's behalf for a measurement -- an observable side effect
           from a harness that is supposed to have none.

           An empty lock means the C++ arm never forced the installable, so
           there is nothing to compare and nothing to be gained by making one
           appear. */
        auto lockedFlake = flake->_lockedFlake;
        if (!lockedFlake) {
            ShadowCensus::skipped(ShadowSkip::FlakeUnservable);
            return std::nullopt;
        }

        auto [lockFileStr, keyMap] = lockedFlake->lockFile.to_string();
        (void) keyMap;

        Strings attrPaths;
        for (auto & candidate : flake->getActualAttrPaths())
            attrPaths.push_back(candidate);

        return RustEvaluand{
            .src =
                RustSource{
                    .source = std::string(flake::callFlakeSource()),
                    .baseDir = "/",
                    .file = "",
                },
            .args =
                {
                    RustArgument{.kind = RustArgument::Kind::Json, .text = nlohmann::json(lockFileStr).dump()},
                    RustArgument{
                        .kind = RustArgument::Kind::Json, .text = flakeOverridesJSON(state, *lockedFlake).dump()},
                    RustArgument{.kind = RustArgument::Kind::InternalPrimop, .text = "fetchFinalTree"},
                },
            .attrPaths = attrPaths,
            .indexLists = false,
            .flakeRef = flake->flakeRef.to_string(),
        };
    } catch (std::exception &) {
        /* Serialising the lock is the one step here that can fail -- a node
           with no key, a source path that is not in the store -- and it fails
           on the shadow arm's own account, after the user has been served.
           Counted rather than raised, and counted under the reason that says
           a flake got this far and still was not compared, because a harness
           whose failures vanish is a harness that reports zero divergences
           for the wrong reason. */
        ShadowCensus::skipped(ShadowSkip::FlakeUnservable);
        return std::nullopt;
    }
}

std::optional<std::string> shadowDerivationText(Store & store, const DerivedPaths & paths)
{
    std::vector<std::pair<StorePath, StringSet>> found;
    for (auto & path : paths) {
        auto * built = std::get_if<DerivedPath::Built>(&path.raw());
        if (!built) {
            /* An `Opaque` derived path is a store path the user named
               directly, which nothing evaluated and there is nothing to
               compare. */
            ShadowCensus::skipped(ShadowSkip::CppAnswerShape);
            return std::nullopt;
        }
        auto * opaque = std::get_if<SingleDerivedPath::Opaque>(&built->drvPath->raw());
        if (!opaque) {
            // A dynamic derivation: the drvPath is itself the output of a
            // build, so it is not a path anything can compare yet.
            ShadowCensus::skipped(ShadowSkip::CppAnswerShape);
            return std::nullopt;
        }
        auto * names = std::get_if<OutputsSpec::Names>(&built->outputs.raw);
        if (!names) {
            // `^*`, whose expansion needs the derivation read back off disk.
            ShadowCensus::skipped(ShadowSkip::CppAnswerShape);
            return std::nullopt;
        }
        // Copied rather than referenced: `OutputsSpec::Names` is a set with a
        // transparent comparator and `StringSet` is not, so they are two
        // types holding the same strings.
        found.emplace_back(opaque->path, StringSet(names->begin(), names->end()));
    }
    if (found.empty()) {
        ShadowCensus::skipped(ShadowSkip::CppAnswerShape);
        return std::nullopt;
    }
    return shadowDerivationLines(store, found);
}

std::vector<RustDerivation> rustEvalDerivations(EvalState & state, const RustEvaluand & evaluand)
{
    RustEvalSetup setup(state);

    IxeSessionRef session(setup);
    if (!session.p) {
        /* The evaluator refused the host it was handed. The one way that
           happens today is a partial set of the five path reads, which
           would leave this evaluator reading outside the process's access
           control; the reason comes back through the setting-conflict slot
           rather than being guessed at here. */
        IxeString why;
        why.s = ixe_take_setting_conflict();
        throw Error(
            "rust-eval: could not create an evaluation session: %s",
            why.s ? why.str() : "the evaluator gave no reason");
    }

    /* One path, as in `rustEvalRender` and for the same reason. `nix build
       <flake>#attr` is the command warm starts were built for, and it is the
       one `mayBeMemoised` excluded. */
    IxeValue root;
    std::string served;
    auto mode = askQuestion(
        state,
        session,
        root,
        served,
        evaluand,
        IXE_QUESTION_DERIVATION,
        /* no rendering happens for this question; the field is in the key
           anyway only for the select shape. */
        IXE_RENDER_PLAIN);
    if (mode == Served::Answer)
        return {decodeDerivation(state, served)};

    selectFrom(state, session, root, evaluand);

    // cppnix's `getDerivations` accepts three shapes here: a derivation, a
    // path or string naming a store path, and an attribute set to recurse
    // into. Only the first is served. The other two are refused by name
    // rather than approximated, because both would change what gets built:
    // `trySinglePathToDerivedPaths` copies a path into the store, and the
    // recursion has `recurseForDerivations` rules of its own.
    if (ixe_value_type(session.p, root.handle) != IXE_TYPE_ATTRS)
        refuse(
            refusalTokens::notADerivation,
            "an installable that is not a derivation (this backend builds a derivation, and "
            "cppnix would also accept a store path or an attribute set to recurse into)");

    {
        IxeValue type;
        if (!selectOptional(state, session, root.handle, "type", type)
            || forceString(state, session, type, "the 'type' attribute") != "derivation")
            refuse(
                refusalTokens::notADerivation,
                "an attribute set that is not a derivation (cppnix would recurse into it "
                "looking for derivations, which this backend does not do)");
    }

    StorePath drvPath = ({
        IxeValue attr;
        if (!selectOptional(state, session, root.handle, "drvPath", attr))
            throw Error("derivation does not contain a 'drvPath' attribute");
        auto text = forceString(state, session, attr, "the 'drvPath' attribute");
        auto path = state.store->parseStorePath(text);
        // cppnix's `requireDerivation`, and its wording. A `drvPath` naming
        // something that is not a derivation would build the wrong thing.
        path.requireDerivation();
        std::move(path);
    });

    // The `outputs` list, then the reduction cppnix's
    // `queryOutputs(false, true)` applies to it.
    StringSet outputs;
    {
        IxeValue attr;
        if (selectOptional(state, session, root.handle, "outputs", attr)) {
            if (int rc = ixe_force(session.p, attr.handle); rc != 0)
                session.fail(state, rc);
            if (ixe_value_type(session.p, attr.handle) != IXE_TYPE_LIST)
                refuse(refusalTokens::notADerivation, "the 'outputs' attribute is not a list");
            size_t count = 0;
            if (int rc = ixe_list_len(session.p, attr.handle, &count); rc != 0)
                session.fail(state, rc);
            for (size_t i = 0; i < count; ++i) {
                IxeValue element;
                IxeHandle handle = 0;
                if (int rc = ixe_list_at(session.p, attr.handle, i, &handle); rc != 0)
                    session.fail(state, rc);
                element.reset(session.p, handle);
                outputs.insert(forceString(state, session, element, "an element of the 'outputs' list"));
            }
        }
        // cppnix's default when there is no `outputs` attribute, and its
        // fallback when the list turned out empty.
        if (outputs.empty())
            outputs.insert("out");
    }

    // `outputSpecified` selects one output by name and is what `lib.getOutput`
    // sets. Refused rather than implemented: it reads `outputName`, which is
    // another attribute and another rule, and nothing this backend serves
    // today produces it.
    {
        IxeValue attr;
        if (selectOptional(state, session, root.handle, "outputSpecified", attr)) {
            if (int rc = ixe_force(session.p, attr.handle); rc != 0)
                session.fail(state, rc);
            if (ixe_value_type(session.p, attr.handle) != IXE_TYPE_BOOL)
                refuse(refusalTokens::outputsToInstall, "'outputSpecified' is not a boolean");
            IxeString rendered;
            if (int rc = ixe_render(session.p, attr.handle, IXE_RENDER_RAW, &rendered.s); rc != 0)
                session.fail(state, rc);
            if (rendered.str() == "true")
                refuse(
                    refusalTokens::outputsToInstall, "'outputSpecified = true', which selects a single output by name");
        }
    }

    // `meta.outputsToInstall`, which nixpkgs sets on most packages: without
    // it a multi-output package would build every output where cppnix builds
    // the named ones, which is a different build rather than a missing
    // feature. Only the plain shape is reduced -- a list of strings, each of
    // them an output this derivation has. Anything else is refused, because
    // cppnix's `checkMeta` has rules for when a bad value falls back to the
    // full set, and mirroring those rules here would be a second
    // implementation of them for the two to disagree over.
    {
        IxeValue meta;
        if (selectOptional(state, session, root.handle, "meta", meta)) {
            if (int rc = ixe_force(session.p, meta.handle); rc != 0)
                session.fail(state, rc);
            if (ixe_value_type(session.p, meta.handle) != IXE_TYPE_ATTRS)
                refuse(refusalTokens::outputsToInstall, "'meta' is not an attribute set");
            IxeValue wanted;
            if (selectOptional(state, session, meta.handle, "outputsToInstall", wanted)) {
                if (int rc = ixe_force(session.p, wanted.handle); rc != 0)
                    session.fail(state, rc);
                if (ixe_value_type(session.p, wanted.handle) != IXE_TYPE_LIST)
                    refuse(refusalTokens::outputsToInstall, "'meta.outputsToInstall' is not a list");
                size_t count = 0;
                if (int rc = ixe_list_len(session.p, wanted.handle, &count); rc != 0)
                    session.fail(state, rc);
                StringSet reduced;
                for (size_t i = 0; i < count; ++i) {
                    IxeValue element;
                    IxeHandle handle = 0;
                    if (int rc = ixe_list_at(session.p, wanted.handle, i, &handle); rc != 0)
                        session.fail(state, rc);
                    element.reset(session.p, handle);
                    auto name = forceString(state, session, element, "an element of 'meta.outputsToInstall'");
                    if (!outputs.contains(name))
                        refuse(
                            refusalTokens::outputsToInstall,
                            "'meta.outputsToInstall' names '%s', which is not one of this "
                            "derivation's outputs",
                            name);
                    reduced.insert(std::move(name));
                }
                // An empty list would build nothing at all. cppnix reduces to
                // it happily; this refuses, because a build that silently
                // produces no output is the shape nobody notices.
                if (reduced.empty())
                    refuse(refusalTokens::outputsToInstall, "'meta.outputsToInstall' is empty");
                outputs = std::move(reduced);
            }
        }
    }

    RustDerivation found{.drvPath = std::move(drvPath), .outputs = std::move(outputs)};
    reportAnswer(session, encodeDerivation(state, found));
    /* The served answer wins a sampled check, for the reason `rustEvalSelect`
       returns it: what gets built must not depend on whether the verifier
       picked this run. A disagreement is an error-priority complaint out of
       the evaluator, which is the part that must be seen. */
    if (mode == Served::Verify)
        return {decodeDerivation(state, served)};
    return {std::move(found)};
}

#else

/* An empty context object, so `RustEvalSetup`'s `unique_ptr` member has a
   complete type to destroy in this arm too. Nothing reaches `host()` here:
   every caller of it is inside the `#if` above. */
struct RustEvalHost
{};

RustEvalSetup::RustEvalSetup(EvalState &) {}

RustEvalSetup::~RustEvalSetup() {}

const IxeHostVtable * RustEvalSetup::host() const
{
    return nullptr;
}

void rustEvalThrow(EvalState &, int, const std::string &, std::string_view, std::shared_ptr<const Pos>)
{
    throw Error("this nix was built without the rust evaluator (meson -Drust-eval=true)");
}

std::shared_ptr<const Pos> rustEvalPos(EvalState &, const std::string &, const char *, uint32_t, uint32_t)
{
    throw Error("this nix was built without the rust evaluator (meson -Drust-eval=true)");
}

/* The parameter list has to match the declaration exactly, defaulted
   arguments included: `nix eval` calls this with six arguments and
   `nix-instantiate` with five, so a stub that drops the trailing `bool`
   satisfies one caller and leaves the other undefined at link time. That is
   what happened -- `-Drust-eval` defaults to `disabled`, so every default
   build of this tree failed linking `src/nix/nix` with "undefined symbol:
   nix::rustEvalSelect(..., RustRender, bool)". */
std::string rustEvalSelect(
    EvalState &, const std::string &, const std::string &, const std::string &, const std::string &, RustRender, bool)
{
    throw Error("this nix was built without the rust evaluator (meson -Drust-eval=true)");
}

std::string shadowErrorClass(const std::exception &)
{
    return "no-rust-eval";
}

/* Silent and counted, not a throw. `eval-backend = shadow` is refused by
   `EvalState`'s experimental-feature check long before anything reaches here,
   so this is unreachable in practice -- but a stub that threw would turn the
   one shape shadow promises never to break (the served command) into a
   failure if it ever were reached. The skip keeps that promise and leaves a
   count behind. */
void rustEvalShadow(EvalState &, const ShadowSubject &, const ShadowCppOutcome &)
{
    ShadowCensus::skipped(ShadowSkip::BackendAbsent);
}

std::optional<RustEvaluand> shadowEvaluandOfInstallable(EvalState &, Installable &)
{
    ShadowCensus::skipped(ShadowSkip::BackendAbsent);
    return std::nullopt;
}

/* No skip, and that is not an omission. This one only converts an answer the
   caller already has into the bytes a comparison would use; the caller stops
   at `shadowEvaluandOfInstallable`, which counted the skip. Counting again
   here would report two uncompared evaluations where there was one. */
std::optional<std::string> shadowDerivationText(Store &, const DerivedPaths &)
{
    return std::nullopt;
}

std::vector<RustDerivation> rustEvalDerivations(EvalState &, const RustEvaluand &)
{
    throw Error("this nix was built without the rust evaluator (meson -Drust-eval=true)");
}

RustEvaluand rustEvaluandOf(SourceExprCommand &, ref<EvalState>, const std::optional<RustSource> &, std::string_view)
{
    throw Error("this nix was built without the rust evaluator (meson -Drust-eval=true)");
}

std::string rustEvalRender(EvalState &, const RustEvaluand &, RustRender, bool)
{
    throw Error("this nix was built without the rust evaluator (meson -Drust-eval=true)");
}

#endif

} // namespace nix
