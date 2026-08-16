#pragma once
/**
 * @file
 *
 * A hook fired on every read performed through `SourcePath`, so that a
 * consumer above `libutil` can record the set of inputs an evaluation
 * observed. Used by the read-set instrumentation in `libexpr`
 * (`nix/expr/eval-readset.hh`), which is what installs the hook.
 *
 * The hook is a plain function pointer rather than a `std::function`
 * because it sits on the path of every file read and the null check has
 * to be free when instrumentation is off, which is always by default.
 */

#include <atomic>
#include <cstdint>
#include <string_view>

namespace nix {

struct SourceAccessor;
struct CanonPath;

/**
 * What kind of observation a recorded read is. The distinction matters
 * for invalidation: adding a file to a directory changes a `listing`
 * without changing any `contents`, so a consumer that only tracked
 * contents would miss it.
 */
enum class SourceReadKind : uint8_t {
    /** The bytes of a file. */
    contents,
    /** The entries of a directory (`readDir`). */
    listing,
    /** Existence or type of a path (`pathExists`, `lstat`, `readFileType`). */
    metadata,
    /** The target of a symlink (`readLink`). */
    link,
    /** A whole subtree, serialised (`dumpPath`, `filterSource`). */
    subtree,
    /** The location of an expression, observed via `unsafeGetAttrPos`. */
    position,
};

using SourceReadHook = void (*)(SourceAccessor &, const CanonPath &, SourceReadKind);

/**
 * Null unless read-set instrumentation is enabled. Written once during
 * `EvalState` construction, before any evaluation thread exists.
 */
extern std::atomic<SourceReadHook> sourceReadHook;

inline void recordSourceRead(SourceAccessor & accessor, const CanonPath & path, SourceReadKind kind)
{
    if (auto hook = sourceReadHook.load(std::memory_order_relaxed)) [[unlikely]]
        hook(accessor, path, kind);
}

/**
 * Fired after a read completes, carrying what was observed: the bytes of a
 * file, the type of a path, the entries of a directory, the target of a
 * symlink, or the serialisation of a subtree.
 *
 * Separate from `SourceReadHook` because a read that throws is still an
 * observation of the input, so the fact of the read is recorded before it and
 * the answer only afterwards. Both are needed: a consumer that records only
 * that a `stat` happened cannot tell an unchanged answer from an answer it
 * never compared, and one that records only a path for a whole subtree reads
 * a changed tree as unchanged.
 */
using SourceObservedHook = void (*)(SourceAccessor &, const CanonPath &, SourceReadKind, std::string_view);

extern std::atomic<SourceObservedHook> sourceObservedHook;

/** Whether anything is listening, for a caller that must do extra work to produce the value. */
inline bool wantSourceObserved()
{
    return sourceObservedHook.load(std::memory_order_relaxed) != nullptr;
}

inline void
recordSourceObserved(SourceAccessor & accessor, const CanonPath & path, SourceReadKind kind, std::string_view observed)
{
    if (auto hook = sourceObservedHook.load(std::memory_order_relaxed)) [[unlikely]]
        hook(accessor, path, kind, observed);
}

} // namespace nix
