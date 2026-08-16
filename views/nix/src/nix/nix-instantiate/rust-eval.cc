#include "rust-eval.hh"
#include "../rust-eval-session.hh"
#include "nix/expr/rust-eval-refusal.hh"

#include "nix/util/error.hh"
#include "nix/expr/eval-error.hh"
#include "nix/expr/eval.hh"

#include <iostream>

// Spelled `defined(...) &&` rather than a bare `#if`: -Werror=undef makes
// an undefined macro a build error, and the whole point of the #else path
// is to compile when -Drust-eval is off and the macro does not exist.
#if defined(HAVE_RUST_EVAL) && HAVE_RUST_EVAL
#  include "ixe.h"
#endif

namespace nix {

#if defined(HAVE_RUST_EVAL) && HAVE_RUST_EVAL

/// Owns the string the C ABI returns so every exit path frees it.
struct IxeString
{
    char * s = nullptr;

    ~IxeString()
    {
        ixe_string_free(s);
    }

    std::string str() const
    {
        return s ? std::string(s) : std::string();
    }
};

/// nix-instantiate.cc's `OutputKind`, which is local to that file. Repeated
/// rather than shared because moving it into the header would put a
/// nix-instantiate detail in front of every other caller of this bridge; the
/// cost is that the two have to be changed together, and the values are
/// checked against it below.
enum OutputKindMirror { okPlain = 0, okRaw = 1, okXML = 2, okJSON = 3 };

static void
rustEvalWhole(EvalState & state, const std::string & source, const std::string & baseDir, const std::string & file);

void rustEvalPrint(
    EvalState & state,
    const std::string & source,
    const std::string & baseDir,
    const std::string & file,
    const Strings & attrPaths,
    int outputKind,
    bool xmlLocation,
    bool strict)
{
    // One refusal left of the four this bridge started with, and one shape of
    // a fifth. Lazy top-level printing needs the printer to stop at a thunk,
    // which this one does not do. `--xml` is served through builtins.toXML's
    // walker -- cppnix's printValueAsXML is one function for both -- but that
    // document has no source positions, so the location-bearing spelling
    // (the default; `--no-location` is what turns it off) is the part still
    // refused rather than answered without the attributes cppnix would print.
    if (outputKind == okXML && xmlLocation)
        refuse(refusalTokens::xmlOutput, "--xml with source locations (run with --no-location)");
    if (!strict)
        refuse(refusalTokens::lazyPrint, "lazy top-level printing (run with --strict)");
    if (outputKind != okPlain && outputKind != okRaw && outputKind != okJSON && outputKind != okXML)
        throw Error("rust-eval: unknown output kind %d", outputKind);

    auto render = outputKind == okJSON  ? RustRender::Json
                  : outputKind == okRaw ? RustRender::Raw
                  : outputKind == okXML ? RustRender::Xml
                                        : RustRender::Plain;

    for (auto & attrPath : attrPaths) {
        // The whole expression, printed plainly, is the one shape whose answer
        // is a string before the caller says anything else about it, so it
        // takes the one-call path -- which is the memoised one. Everything
        // else has to walk the value, and a walk has no answer to memoise
        // (ENG-12470).
        if (attrPath.empty() && render == RustRender::Plain) {
            rustEvalWhole(state, source, baseDir, file);
            continue;
        }
        auto text = rustEvalSelect(state, source, baseDir, file, attrPath, render);
        if (render == RustRender::Raw || render == RustRender::Xml)
            // Deliberately no newline. For raw, matching cppnix: the default
            // Bash PS1 on NixOS opens with one. For XML because the document
            // already ends with one -- XMLWriter closes the root element with
            // a newline of its own, and cppnix's okXML branch appends nothing.
            std::cout << text;
        else
            std::cout << text << std::endl;
    }
}

/// The one-call path: evaluate and render in a single crossing, which is what
/// lets the result cache serve the whole thing on a second run.
static void
rustEvalWhole(EvalState & state, const std::string & source, const std::string & baseDir, const std::string & file)
{
    RustEvalSetup setup(state);

    IxeString out;
    const char * token = nullptr;
    IxePos at = {nullptr, 0, 0};
    int rc = ixe_eval_expr(
        // The host this evaluation answers through. Per call rather than
        // installed on the process, so this crossing cannot pick up or hand
        // on another one's hooks.
        setup.host(),
        reinterpret_cast<const unsigned char *>(source.data()),
        source.size(),
        reinterpret_cast<const unsigned char *>(baseDir.data()),
        baseDir.size(),
        // Empty means no file: `data()` on an empty std::string is non-null,
        // so the pointer rather than the length carries "there is no file".
        file.empty() ? nullptr : reinterpret_cast<const unsigned char *>(file.data()),
        file.size(),
        &out.s,
        &token,
        &at);
    if (rc == 0) {
        std::cout << out.str() << "\n";
        return;
    }
    // One mapping from a status to an exception, shared with the handle path,
    // rather than a second switch that can drift from it. It also throws on
    // status 4, which the switch this replaces did not: `case 4: break;` left
    // the switch and returned normally, so a bad call into nix-eval-rs
    // printed nothing and exited 0.
    //
    // The token is passed rather than defaulted. This path used to have none
    // to give, so it fell back to the `unrecorded` sentinel -- and because it
    // is the path `nix-instantiate --eval` takes for a whole expression, that
    // sentinel was most of the fleet's refusal census (ENG-12819).
    // Built and the path freed before the throw, because rustEvalThrow does
    // not return: `at.file` is this call's to release and there is no scope
    // left to do it in afterwards.
    auto pos = rustEvalPos(state, source, at.file, at.line, at.column);
    ixe_string_free(at.file);
    rustEvalThrow(state, rc, out.str(), token ? std::string_view(token) : unrecordedRefusal, pos);
}

#else

void rustEvalPrint(
    EvalState &, const std::string &, const std::string &, const std::string &, const Strings &, int, bool, bool)
{
    throw Error("this nix was built without the rust evaluator (meson -Drust-eval=true)");
}

#endif

} // namespace nix
