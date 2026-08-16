#pragma once
/**
 * @file
 *
 * Retained boundary entries: the invalidation prototype the read-set design
 * calls phase 3, cut down to the one boundary that carries the cpu.
 *
 * A completed request leaves behind its tracked-entry graph together with the
 * fully forced result of every `derivationStrict` call. A later request for
 * the same installable walks that graph from the inputs that changed, marks
 * the reachable entries dirty, and answers every clean `derivationStrict`
 * call from the retained value without forcing the derivation attributes,
 * which is where 87% of attributed evaluation cpu sits.
 *
 * This reuses only derivation results, never imports or option values: a
 * derivation result is a flat attribute set of fully forced strings, so
 * splicing it cannot trigger evaluation in a previous request environment.
 * An import or option value can hold unforced thunks whose later forcing
 * would read the previous tree, so those stay unspliced.
 *
 * Value flows are carried by the provenance side table in the tracker: a
 * string literal, tree version attribute or readFile result that crosses the
 * module fixpoint into a derivation argument records the file or attribute it
 * came from in the consuming entry's own input set, which closed the 38 of
 * 1,316 splices the first prototype served stale. What still escapes is a
 * value whose payload is inline in the 16-byte Value (an integer or boolean
 * not rendered inside a tracked string operation and not co-located with any
 * consumed string from the same file), influence through control flow alone,
 * and `fromTOML`. Those classes are why the flag still defaults off and why
 * the fresh-process comparison remains the only accepted evidence.
 */

#include "nix/expr/value.hh"
#include "nix/expr/eval-gc.hh"

#include <cstdint>
#include <cstdio>
#include <map>
#include <optional>
#include <string>
#include <vector>

namespace nix {

class EvalState;
class ReadSetTracker;
enum class TrackedEntryKind : uint8_t;

/** One tree as a completed request saw it, named so two requests can pair them. */
struct RetainedTree
{
    std::string identity;
    std::string view;
    std::string fp;
    std::string root;
};

/** One input as recorded: what kind of observation, of which tree, and what it answered. */
struct RetainedInput
{
    std::string kind;
    int32_t tree = -1;
    std::string rel;
    std::string path;
    /** What the read observed, filled at extraction from the observation table. */
    std::string observed;
};

struct RetainedEntry
{
    uint64_t id;
    TrackedEntryKind kind;
    std::string key;
    /** The tree the entry accessor answers for, or -1. Import keys are tree-relative paths, so two trees share them. */
    int32_t tree = -1;
    std::vector<uint32_t> inputs;
    /** Ids of the entries whose values flowed into this one. */
    std::vector<uint64_t> edges;
    /** The derivation store path this entry produced, empty for non-derivations. */
    std::string produced;
    /**
     * The `derivationStrict` result, a fully forced attrset of strings.
     * Kept alive by `RetainedEval::rootedValues`.
     */
    Value * value = nullptr;
};

/**
 * The graph and values one completed request retained, and the dirty-set
 * walk that decides which of them a later request may reuse.
 */
class RetainedEval
{
public:
    /** Indexed by entry id: ids are assigned densely from zero. */
    std::vector<RetainedEntry> entries;
    /** Indexed by input id. */
    std::vector<RetainedInput> inputs;
    /** Indexed by tree id. */
    std::vector<RetainedTree> trees;

    struct Stats
    {
        uint64_t spliced = 0;
        uint64_t dirtySeeds = 0;
        uint64_t dirtyEntries = 0;
        uint64_t movedTrees = 0;
        uint64_t verifiedFiles = 0;
        uint64_t lookupMisses = 0;
        uint64_t dirtyRefused = 0;
    };

    Stats stats;

    /**
     * When set, one line per splice and per dirty refusal is appended here.
     * The splice lines carry the produced store path, so a comparison against
     * a fresh-process trace of the same tree names every stale splice: a
     * produced path the fresh evaluation did not produce is a wrong answer in
     * the making, attributed to its entry.
     */
    std::FILE * spliceLog = nullptr;

    /** Build the name and consumer indexes once the vectors above are filled. */
    void buildIndexes();

    /** Root a derivation result so a later request can splice it. */
    void retainValue(uint64_t entryId, Value * v);

    /** Reset per-request state: correspondence, stats, the prepared dirty set. */
    void beginRequest();

    /** What resolving a derivation against the previous graph concluded. */
    struct Resolved
    {
        const RetainedEntry * entry = nullptr;
        /** Whether the dirty walk condemned it, in which case it must be recomputed. */
        bool dirty = true;
    };

    /**
     * The old entry this derivation call corresponds to, found by name among
     * the recorded edges of the old entry the demanding parent corresponds
     * to. Identity flows along the edges rather than a global per-name
     * counter, because splicing changes which calls happen at all: a counter
     * pairs the nth call with the nth old entry, and the first spliced
     * parent that stops demanding a child shifts every later pair one off,
     * which served a same-named derivation with different outputs. An entry
     * resolves at most once per request.
     */
    Resolved resolveDerivation(std::string_view drvName, int64_t parentOldId, ReadSetTracker & current);

    /**
     * The old id of the import entry with this key in the tree of this
     * identity and view, or -1. Import keys are paths within the answering
     * tree, so the tree is part of the name; a key that is still ambiguous
     * resolves to nothing.
     */
    int64_t importIdFor(std::string_view identity, std::string_view view, std::string_view key) const;

    /** Identity and view of a tree in this graph, for building import keys. */
    static std::string importKey(std::string_view identity, std::string_view view, std::string_view key);

private:
    void prepare(ReadSetTracker & current);

    bool prepared = false;
    std::vector<bool> dirty;
    /** Import entry key to id, or -1 for a key two trees both hold. */
    std::map<std::string, int64_t, std::less<>> importByKey;
    /** Old entries already resolved this request, so one cannot splice twice. */
    std::vector<bool> resolved;
    /** Reverse edges: entry id to the ids of the entries that consumed its value. */
    std::vector<std::vector<uint64_t>> consumers;

    /**
     * The GC root for every retained value. The values were kept alive during
     * their own request by the evaluator, and by this vector afterwards; the
     * allocator is what makes the collector scan it.
     */
    std::vector<Value *, traceable_allocator<Value *>> rootedValues;
};

} // namespace nix
