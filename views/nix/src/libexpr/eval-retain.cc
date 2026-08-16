#include "nix/expr/eval-retain.hh"
#include "nix/expr/eval-readset.hh"
#include "nix/util/file-system.hh"
#include "nix/util/hash.hh"

#include <algorithm>
#include <filesystem>

namespace nix {

static const RetainedTree emptyTree;

std::string RetainedEval::importKey(std::string_view identity, std::string_view view, std::string_view key)
{
    std::string out;
    out.reserve(identity.size() + view.size() + key.size() + 2);
    out += identity;
    out += 1;
    out += view;
    out += 1;
    out += key;
    return out;
}

void RetainedEval::buildIndexes()
{
    std::sort(entries.begin(), entries.end(), [](const auto & a, const auto & b) { return a.id < b.id; });

    consumers.assign(entries.size(), {});
    for (const auto & e : entries) {
        if (e.kind == TrackedEntryKind::import && !e.key.empty()) {
            auto & t = e.tree >= 0 && size_t(e.tree) < trees.size() ? trees[size_t(e.tree)] : emptyTree;
            auto treeKey = importKey(t.identity, t.view, e.key);
            /* An import key is the path within whichever tree answered, and
               different trees hold the same paths: every flake has a
               /flake.nix. A name two entries share identifies neither, and
               resolving it to the first seen handed nixpkgs imports the ix
               flake as their counterpart, whose same-named edge children then
               spliced values from the wrong tree. Collisions resolve to
               nothing instead. */
            auto [it, inserted] = importByKey.emplace(treeKey, int64_t(e.id));
            if (!inserted)
                it->second = -1;
        }
        for (auto producer : e.edges)
            if (producer < consumers.size())
                consumers[producer].push_back(e.id);
    }
}

void RetainedEval::retainValue(uint64_t entryId, Value * v)
{
    if (entryId < entries.size() && entries[entryId].id == entryId) {
        entries[entryId].value = v;
        rootedValues.push_back(v);
    }
}

void RetainedEval::beginRequest()
{
    prepared = false;
    dirty.clear();
    resolved.assign(entries.size(), false);
    stats = Stats{};
}

/**
 * The content hash exactly as `ReadSetTracker::recordObserved` writes it, so
 * that a hash computed here compares against a recorded observation.
 */
static std::string observedContentHash(std::string_view bytes)
{
    return hashString(HashAlgorithm::SHA256, bytes).to_string(HashFormat::Base16, false).substr(0, 32);
}

/**
 * Reproduce the observation string the tracker records for a stat, exactly
 * as `showStatObservation` in libutil writes it. Reproducing the string is
 * what lets a stat under an edited tree validate instead of counting as
 * dirty just because the tree moved.
 */
static std::string statObservation(const std::filesystem::path & p)
{
    std::error_code ec;
    auto st = std::filesystem::symlink_status(p, ec);
    if (ec || st.type() == std::filesystem::file_type::not_found)
        return "absent";
    using ft = std::filesystem::file_type;
    auto t = st.type();
    if (t == ft::regular)
        return (st.permissions() & std::filesystem::perms::owner_exec) != std::filesystem::perms::none
                   ? "regular,executable"
                   : "regular";
    if (t == ft::directory)
        return "directory";
    if (t == ft::symlink)
        return "symlink";
    if (t == ft::character)
        return "character device";
    if (t == ft::block)
        return "block device";
    if (t == ft::socket)
        return "socket";
    if (t == ft::fifo)
        return "fifo";
    return "unknown";
}

/** The type word `SourceAccessor::Stat::typeString` uses for a listing entry. */
static std::string_view listingTypeString(std::filesystem::file_type t)
{
    using ft = std::filesystem::file_type;
    if (t == ft::regular)
        return "regular";
    if (t == ft::directory)
        return "directory";
    if (t == ft::symlink)
        return "symlink";
    return "unknown";
}

/**
 * Reproduce the listing observation: the sorted names and their types, one
 * `name:type` per line, as `SourcePath::readDirectory` records it.
 */
static std::optional<std::string> listingObservation(const std::filesystem::path & p)
{
    std::error_code ec;
    std::vector<std::pair<std::string, std::filesystem::file_type>> entries;
    for (auto it = std::filesystem::directory_iterator(p, ec); !ec && it != std::filesystem::directory_iterator();
         it.increment(ec)) {
        auto st = it->symlink_status(ec);
        if (ec)
            return std::nullopt;
        entries.emplace_back(it->path().filename().string(), st.type());
    }
    if (ec)
        return std::nullopt;
    std::sort(entries.begin(), entries.end());
    std::string observed;
    for (auto & [name, type] : entries) {
        observed += name;
        observed += 58;
        observed += listingTypeString(type);
        observed += 10;
    }
    return observed;
}

/**
 * The value `recordObserved` stores for an observation this long: short
 * answers go in as themselves, anything longer is hashed, and file contents
 * are always hashed.
 */
static std::string storedObservation(std::string_view observed, bool isContents)
{
    if (observed.size() <= 32 && !isContents)
        return std::string(observed);
    return hashString(HashAlgorithm::SHA256, observed).to_string(HashFormat::Base16, false).substr(0, 32);
}

/** Strip the `:line:column` a position input carries after its file path. */
static std::string_view positionFile(std::string_view rel)
{
    auto second = rel.rfind(":");
    if (second == std::string_view::npos)
        return rel;
    auto first = rel.rfind(":", second - 1);
    if (first == std::string_view::npos)
        return rel;
    auto digits = [&](size_t from, size_t to) {
        if (from >= to)
            return false;
        for (auto i = from; i < to; ++i)
            if (rel[i] < 48 || rel[i] > 57)
                return false;
        return true;
    };
    if (digits(first + 1, second) && digits(second + 1, rel.size()))
        return rel.substr(0, first);
    return rel;
}

void RetainedEval::prepare(ReadSetTracker & current)
{
    prepared = true;
    dirty.assign(entries.size(), false);

    /* Pair this graph trees against the ones the running request has fetched
       so far. A tree the new request has not touched yet cannot be verified
       and counts as moved, which recomputes its readers rather than trusting
       them. */
    /* Later sightings supersede earlier ones: the literal replay at tracker
       construction re-interns trees through the previous request's
       accessors, so the first record of the tree under edit carries the
       previous fingerprint. The re-fetch that follows is the tree as it
       stands, and pairing against the stale record made an edited tree read
       as unmoved, which spliced the whole evaluation stale at 15.2% of
       cold. */
    std::map<std::pair<std::string, std::string>, const RetainedTree *> currentByIdentity;
    for (const auto & t : current.liveTrees())
        currentByIdentity.insert_or_assign(std::make_pair(t.identity, t.view), &t);

    /* Whether every flake.lock this graph read still holds the same bytes.
       When it does, a tree the new request has not fetched by the time the
       first splice is attempted can only be one of the locked inputs, whose
       revision the unchanged lock file still names, so treating it as moved
       would dirty its readers for no reason. The verification below runs
       before the per-tree loop because the verdict feeds it. */
    bool locksUnchanged = true;
    for (const auto & in : inputs) {
        if (in.kind != "contents" || !in.rel.ends_with("/flake.lock"))
            continue;
        auto it = currentByIdentity.find(
            in.tree >= 0 && size_t(in.tree) < trees.size()
                ? std::make_pair(trees[size_t(in.tree)].identity, trees[size_t(in.tree)].view)
                : std::make_pair(std::string(), std::string()));
        std::string root = it != currentByIdentity.end() ? it->second->root : "";
        if (root.empty() || in.observed.empty()) {
            locksUnchanged = false;
            break;
        }
        try {
            if (storedObservation(readFile(std::filesystem::path(root + in.rel)), true) != in.observed) {
                locksUnchanged = false;
                break;
            }
        } catch (...) {
            locksUnchanged = false;
            break;
        }
    }

    std::vector<bool> treeMoved(trees.size(), true);
    std::vector<std::string> currentRoot(trees.size());
    std::vector<std::string> movedStoreRoots;
    bool anyMoved = false;
    for (size_t i = 0; i < trees.size(); ++i) {
        auto it = currentByIdentity.find(std::make_pair(trees[i].identity, trees[i].view));
        if (it != currentByIdentity.end()) {
            treeMoved[i] = it->second->fp != trees[i].fp;
            currentRoot[i] = it->second->root;
        } else if (locksUnchanged) {
            /* Not fetched yet this request. With every lock file unchanged
               the pinned revision cannot have moved, and if the tree is
               never fetched at all, nothing spliced from it can be forced
               into reading it. */
            treeMoved[i] = false;
        }
        if (treeMoved[i]) {
            anyMoved = true;
            stats.movedTrees++;
            if (trees[i].root.starts_with("/nix/store/"))
                movedStoreRoots.push_back(trees[i].root);
        }
    }

    /* Where an unchanged file sat in this graph run, so a position input can
       be validated by the bytes of the file it points into. */
    std::map<std::pair<int32_t, std::string_view>, std::string_view> contentHashByFile;
    for (const auto & in : inputs)
        if (in.kind == "contents" && !in.observed.empty())
            contentHashByFile.emplace(std::make_pair(in.tree, std::string_view(in.rel)), in.observed);

    auto fileUnchanged = [&](int32_t tree, std::string_view rel) {
        if (tree < 0 || size_t(tree) >= trees.size())
            return false;
        if (!treeMoved[size_t(tree)])
            return true;
        auto & root = currentRoot[size_t(tree)];
        if (root.empty())
            return false;
        auto recorded = contentHashByFile.find(std::make_pair(tree, rel));
        if (recorded == contentHashByFile.end())
            return false;
        try {
            stats.verifiedFiles++;
            auto bytes = readFile(std::filesystem::path(root + std::string(rel)));
            return observedContentHash(bytes) == recorded->second;
        } catch (...) {
            return false;
        }
    };

    /* Whether a read of a moved tree still observes what this graph
       recorded, by redoing the read against the moved tree on disk and
       reproducing the recorded serialisation. An input that cannot be
       re-observed, because the root is not on disk or the observation was
       never recorded, counts as dirty: the cost of that is recomputation,
       never a stale answer. */
    auto reObserved = [&](const RetainedInput & in) {
        if (in.tree < 0 || size_t(in.tree) >= trees.size())
            return false;
        if (!treeMoved[size_t(in.tree)])
            return true;
        auto & root = currentRoot[size_t(in.tree)];
        if (root.empty() || in.observed.empty())
            return false;
        stats.verifiedFiles++;
        auto onDisk = std::filesystem::path(root + in.rel);
        try {
            if (in.kind == "contents")
                return storedObservation(readFile(onDisk), true) == in.observed;
            if (in.kind == "metadata")
                return storedObservation(statObservation(onDisk), false) == in.observed;
            if (in.kind == "listing") {
                auto listed = listingObservation(onDisk);
                return listed && storedObservation(*listed, false) == in.observed;
            }
            if (in.kind == "link") {
                std::error_code ec;
                auto target = std::filesystem::read_symlink(onDisk, ec);
                return !ec && storedObservation(target.string(), false) == in.observed;
            }
        } catch (...) {
            return false;
        }
        return false;
    };

    std::vector<bool> inputDirty(inputs.size(), false);
    for (size_t i = 0; i < inputs.size(); ++i) {
        const auto & in = inputs[i];
        if (in.kind == "contents" || in.kind == "metadata" || in.kind == "listing" || in.kind == "link") {
            inputDirty[i] = !reObserved(in);
        } else if (in.kind == "position") {
            inputDirty[i] = !fileUnchanged(in.tree, positionFile(in.rel));
        } else if (in.kind == "tree-attr") {
            /* The input name embeds the flake input spec, and there is no
               cheap join from the spec to a tree record, so the moment any
               tree moved every version attribute is treated as dirty. The
               previous rule dirtied only `file:` specs, which missed the
               `git+file:` spec of the tree actually under edit and served
               `dirtyRev` readers stale. Few entries read version attributes,
               so the over-approximation is cheap, and its cost is
               recomputation, never a stale answer. */
            inputDirty[i] = anyMoved;
        } else if (in.kind == "store") {
            for (const auto & root : movedStoreRoots)
                if (in.path.starts_with(root) && (in.path.size() == root.size() || in.path[root.size()] == 47)) {
                    inputDirty[i] = true;
                    break;
                }
        } else {
            /* subtree: the dump of a whole tree, whose hash moves with any
               edit under it. Genuinely dirty whenever the tree moved. */
            inputDirty[i] = in.tree < 0 || size_t(in.tree) >= trees.size() || treeMoved[size_t(in.tree)];
        }
    }

    std::vector<uint64_t> work;
    for (const auto & e : entries)
        for (auto in : e.inputs)
            if (in < inputDirty.size() && inputDirty[in]) {
                if (!dirty[e.id]) {
                    dirty[e.id] = true;
                    stats.dirtySeeds++;
                    work.push_back(e.id);
                }
                break;
            }

    while (!work.empty()) {
        auto producer = work.back();
        work.pop_back();
        for (auto consumer : consumers[producer])
            if (!dirty[consumer]) {
                dirty[consumer] = true;
                work.push_back(consumer);
            }
    }
    stats.dirtyEntries = size_t(std::count(dirty.begin(), dirty.end(), true));
}

int64_t RetainedEval::importIdFor(std::string_view identity, std::string_view view, std::string_view key) const
{
    auto it = importByKey.find(importKey(identity, view, key));
    return it == importByKey.end() ? -1 : it->second;
}

RetainedEval::Resolved
RetainedEval::resolveDerivation(std::string_view drvName, int64_t parentOldId, ReadSetTracker & current)
{
    if (!prepared)
        prepare(current);

    if (parentOldId < 0 || size_t(parentOldId) >= entries.size()) {
        stats.lookupMisses++;
        return {};
    }

    /* The candidates are the old entries whose values flowed into the old
       counterpart of whoever is demanding now. A parent that did not demand
       this name in the previous run yields no candidate, and the call is
       computed rather than guessed at. */
    const RetainedEntry * found = nullptr;
    bool ambiguous = false;
    for (auto producerId : entries[size_t(parentOldId)].edges) {
        if (producerId >= entries.size())
            continue;
        const auto & producer = entries[producerId];
        if (producer.kind == TrackedEntryKind::derivation && !resolved[producerId] && producer.key == drvName) {
            if (found) {
                /* Two unresolved candidates under one parent share this name,
                   and the edges are stored sorted by id, so demand order
                   within the parent is gone. `source` alone names most
                   fetched trees. Guessing here is how a consumer gets the
                   other source, so compute instead. */
                ambiguous = true;
                break;
            }
            found = &producer;
        }
    }
    if (!found || ambiguous) {
        stats.lookupMisses++;
        return {};
    }
    resolved[found->id] = true;
    if (dirty[found->id]) {
        stats.dirtyRefused++;
        if (spliceLog)
            fprintf(spliceLog, "recompute %llu %s\n", (unsigned long long) found->id, found->key.c_str());
        return {found, true};
    }
    if (!found->value || found->produced.empty())
        return {found, true};
    stats.spliced++;
    if (spliceLog)
        fprintf(
            spliceLog,
            "splice %llu %s %s\n",
            (unsigned long long) found->id,
            found->key.c_str(),
            found->produced.c_str());
    return {found, false};
}

} // namespace nix
