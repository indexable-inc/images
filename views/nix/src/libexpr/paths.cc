#include "nix/store/store-api.hh"
#include "nix/expr/eval.hh"
#include "nix/util/mounted-source-accessor.hh"
#include "nix/fetchers/fetch-to-store.hh"
#include "nix/fetchers/fetchers.hh"
#include "nix/util/file-system.hh"
#include "nix/util/sync.hh"
#include "nix/util/configuration.hh"

#ifdef __APPLE__
#  include <sys/clonefile.h>
#endif

namespace nix {

/* Snapshot a mutable source tree with an in-kernel APFS directory clone.

   A lazily mounted input is read from the live filesystem for the whole
   evaluation, so a `path:` input or a dirty git worktree that another
   process writes to mid-eval makes the on-demand copy hash differently
   from the mount-time dry-run hash and the evaluation dies in
   ensureLazyPathCopied (indexable-inc/index#3749). Cloning the tree at
   mount time (clonefile(2): one blocking syscall, ~70 ms for a
   4k-file/59 MB worktree, content verified identical) and evaluating
   from the clone makes hash and content agree by construction.

   Returns std::nullopt when cloning is unavailable (non-Darwin,
   cross-volume EXDEV, non-APFS); the caller then falls back to copying
   the tree to the store eagerly, which is the pre-lazy-trees base
   behavior. Snapshots are deduplicated per source path and live until
   process exit. */
static std::optional<std::filesystem::path> cloneTreeSnapshot(const std::filesystem::path & src)
{
#ifdef __APPLE__
    static std::filesystem::path snapRoot = createTempDir("", "nix-eval-snapshot");
    static AutoDelete snapRootDelete(snapRoot, true);
    static Sync<std::map<std::filesystem::path, std::filesystem::path>> snapshots_;

    auto snapshots(snapshots_.lock());
    if (auto it = snapshots->find(src); it != snapshots->end())
        return it->second;
    auto dst = snapRoot / fmt("snapshot-%d", snapshots->size());
    if (clonefile(src.c_str(), dst.c_str(), 0) != 0)
        return std::nullopt;
    snapshots->emplace(src, dst);
    return dst;
#else
    (void) src;
    return std::nullopt;
#endif
}

SourcePath EvalState::rootPath(CanonPath path)
{
    return {rootFS, std::move(path)};
}

SourcePath EvalState::rootPath(std::string_view path)
{
    return {rootFS, CanonPath(absPath(path).string())};
}

SourcePath EvalState::storePath(const StorePath & path)
{
    return {rootFS, CanonPath{store->printStorePath(path)}};
}

void EvalState::ensureLazyPathCopied(const StorePath & path)
{
    /* With lazy-trees disabled every input was copied to the store when
       it was mounted, so there is nothing left to materialize. Returning
       early keeps the disabled mode byte-for-byte identical to the
       behavior before this patch series. */
    if (!settings.lazyTrees)
        return;

    if (settings.readOnlyMode)
        return;

    auto mount = storeFS->getMount(CanonPath(store->printStorePath(path)));
    if (!mount)
        return;

    /* TODO: We could memoise this in-memory if necessary. */
    auto storePath = fetchToStore(
        fetchSettings,
        *store,
        SourcePath{ref(mount)},
        /* Force a copy. mountInput does a dryRun to just calculate the storePath and narHash. */
        FetchMode::Copy,
        path.name(),
        /* Ingest with the same method mountInput used, or this re-fetch
           lands on a different store path and the mismatch check below
           misfires. `knownTreeRoot` survives on the accessor exactly when
           the mount was git-CA: mountInput clears it both when the feature
           or a narHash promise rules git-CA out AND when the id's family is
           one nix cannot ingest, so the predicate here stays a presence
           test and the two sites cannot drift apart. */
        mount->knownTreeRoot ? ContentAddressMethod::Raw::Git : ContentAddressMethod::Raw::NixArchive);

    /* This can happen if the source gets modified by another process while we are evaluaing
       from it. Alternatively, the caching might be unsound and fetcher cache is poisoned somehow.
       See https://github.com/NixOS/nix/issues/14317. */
    if (storePath != path) {
        throw Error(
            (unsigned int) 102,
            "store path ('%1%') was hashed to avoid a full copy at first, but upon reading it again, the contents have changed ('%2%'), so we can not proceed. Make sure files do not change during evaluation",
            store->printStorePath(path),
            store->printStorePath(storePath));
    }
}

void EvalState::ensureLazyPathsCopied(const NixStringContext & context)
{
    for (const auto & c : context)
        if (auto * o = std::get_if<NixStringContextElem::Opaque>(&c.raw))
            /* TODO: This could be done in parallel. */
            ensureLazyPathCopied(o->path);
}

StorePath
EvalState::mountInput(fetchers::Input & input, const fetchers::Input & originalInput, ref<SourceAccessor> accessor)
{
    /* With lazy-trees enabled, dryRun is sufficient to mount the input.
       We still compute the narHash (to check for mismatches) and the store
       path to figure out where to mount it, so paths, hashes and lock files
       do not depend on the setting. TODO: This could be relaxed in the future by making outPath and narHash
       lazier. Good code that doesn't do `toString ./.` or otherwise inspects the outPath string and only uses it for
       doing relative imports does not even require computing the store path. That is a big invasive change though and
       would require having a special "LazyStorePathString" thunk. narHash also doesn't need to be computed eagerly in
       case it's not actually specified (like during local development with a dirty tree) - in that case narHash could
       also become a lazy app/thunk that shares the state with the storePath delayed computation. */
    auto mode = settings.lazyTrees ? FetchMode::DryRun : FetchMode::Copy;

    /* Only `path` and `git` (workdir) inputs are backed by trees that
       other processes can mutate; anything else with a physical path
       (say, an archive extracted into the fetcher cache) is owned by
       Nix and stays lazily mounted as-is. The tree root comes from the
       input attrs, not the accessor's physical path: for a git workdir
       the accessor root omits .git, which the re-fetch of the snapshot
       needs. */
    if (mode == FetchMode::DryRun && (input.getType() == "path" || input.getType() == "git")
        && accessor->getPhysicalPath(CanonPath::root)) {
        std::optional<std::filesystem::path> treeRoot;
        if (input.getType() == "path") {
            if (auto phys = accessor->getPhysicalPath(CanonPath::root); phys && phys->is_absolute())
                treeRoot = *phys;
        } else if (auto url = fetchers::maybeGetStrAttr(input.attrs, "url");
                   url && hasPrefix(*url, "file://") && url->size() > 7 && (*url)[7] == '/')
            treeRoot = url->substr(7);
        if (treeRoot && !store->isInStore(treeRoot->string())) {
            bool snapshotted = false;
            if (auto snapshot = cloneTreeSnapshot(*treeRoot)) {
                try {
                    auto attrs = input.attrs;
                    /* Identity and lock attrs describe the original
                       location; mountInput below re-checks the snapshot
                       against the original lock via its narHash. */
                    for (auto & attr :
                         {"narHash", "lastModified", "rev", "revCount", "dirtyRev", "dirtyShortRev", "__final"})
                        attrs.erase(attr);
                    if (input.getType() == "path")
                        attrs.insert_or_assign("path", snapshot->string());
                    else
                        attrs.insert_or_assign("url", "file://" + snapshot->string());
                    accessor = fetchers::Input::fromAttrs(fetchSettings, std::move(attrs))
                                   .getAccessor(fetchSettings, *store)
                                   .first;
                    snapshotted = true;
                } catch (Error & e) {
                    /* e.g. a worktree whose .git file points outside the
                       cloned tree; fall through to the eager copy. */
                    debug("cannot re-fetch snapshot of '%s': %s", treeRoot->string(), e.what());
                }
            }
            if (!snapshotted)
                mode = FetchMode::Copy;
        }
    }

    /* Content-address the mount by the tree id the accessor already knows
       (the jj workdir fetcher announces it): the store path then costs zero
       file reads, where the NAR method re-reads all of a 14k+-file tree on
       every source edit. Only for ids whose family nix ingests, and only
       for inputs that made no narHash promise -- a locked input's narHash
       can only be checked by NAR-ingesting the tree. */
    auto method = ContentAddressMethod::Raw::NixArchive;
    if (accessor->knownTreeRoot) {
        if (accessor->knownTreeRoot->family == KnownTreeRoot::Family::Git
            && experimentalFeatureSettings.isEnabled(Xp::GitHashing) && !originalInput.getNarHash())
            method = ContentAddressMethod::Raw::Git;
        else
            /* Drop the hint so ensureLazyPathCopied re-fetches this mount
               with the same (NAR) method it is being created with. This is
               also the path a family nix cannot ingest takes: the id stays
               true, but it is not a store path address, so it must not
               reach a consumer that would treat it as one. */
            accessor->knownTreeRoot.reset();
    }

    auto [storePath, hash] = fetchToStore2(fetchSettings, *store, accessor, mode, input.getName(), method);

    allowPath(storePath); // FIXME: should just whitelist the entire virtual store

    storeFS->mount(CanonPath(store->printStorePath(storePath)), accessor);

    if (method == ContentAddressMethod::Raw::NixArchive) {
        input.attrs.insert_or_assign("narHash", hash.to_string(HashFormat::SRI, true));

        if (originalInput.getNarHash() && hash != *originalInput.getNarHash())
            throw Error(
                (unsigned int) 102,
                "NAR hash mismatch in input '%s', expected '%s' but got '%s'",
                originalInput.to_string(),
                hash.to_string(HashFormat::SRI, true),
                originalInput.getNarHash()->to_string(HashFormat::SRI, true));
    }

    return storePath;
}

} // namespace nix
