#include <regex>

#include "nix/store/local-overlay-store.hh"
#include "nix/util/callback.hh"
#include "nix/util/file-system.hh"
#include "nix/util/os-string.hh"
#include "nix/store/realisation.hh"
#include "nix/util/processes.hh"
#include "nix/util/url.hh"
#include "nix/store/store-open.hh"
#include "nix/store/store-registration.hh"

namespace nix {

std::string LocalOverlayStoreConfig::doc()
{
    return
#include "local-overlay-store.md"
        ;
}

ref<Store> LocalOverlayStoreConfig::openStore() const
{
    return make_ref<LocalOverlayStore>(
        ref{std::dynamic_pointer_cast<const LocalOverlayStoreConfig>(shared_from_this())});
}

StoreReference LocalOverlayStoreConfig::getReference() const
{
    return {
        .variant =
            StoreReference::Specified{
                .scheme = *uriSchemes().begin(),
            },
    };
}

std::filesystem::path LocalOverlayStoreConfig::toUpperPath(const StorePath & path) const
{
    return upperLayer.get() / path.to_string();
}

LocalOverlayStore::LocalOverlayStore(ref<const Config> config)
    : Store{*config}
    , LocalFSStore{*config}
    , LocalStore{static_cast<ref<const LocalStore::Config>>(config)}
    , config{config}
    , lowerStore(openStore(config->lowerStoreUri.get()).dynamic_pointer_cast<LocalFSStore>())
{
    if (config->checkMount.get()) {
        std::smatch match;
        std::string mountInfo;
        auto mounts = readFile(std::filesystem::path{"/proc/self/mounts"});
        auto regex = std::regex(R"((^|\n)overlay )" + config->realStoreDir.get().string() + R"( .*(\n|$))");

        // Mount points can be stacked, so there might be multiple matching entries.
        // Loop until the last match, which will be the current state of the mount point.
        while (std::regex_search(mounts, match, regex)) {
            mountInfo = match.str();
            mounts = match.suffix();
        }

        auto checkOption = [&](std::string_view option, const std::filesystem::path & value) {
            return std::regex_search(mountInfo, std::regex("\\b" + option + "=" + value.string() + "( |,)"));
        };

        auto expectedLowerDir = lowerStore->config.realStoreDir.get();
        if (!checkOption("lowerdir", expectedLowerDir) || !checkOption("upperdir", config->upperLayer.get())) {
            debug("expected lowerdir: %s", PathFmt(lowerStore->config.realStoreDir.get()));
            debug("expected upperdir: %s", PathFmt(config->upperLayer.get()));
            debug("actual mount: %s", mountInfo);
            throw Error("overlay filesystem %s mounted incorrectly", PathFmt(config->realStoreDir.get()));
        }
    }
}

void LocalOverlayStore::ensureUpperValidPath(const StorePath & path)
{
    /* Copy a lower store object's registration up into this store's own
       database, along with everything it references.

       This is the same copy-up `isValidPathUncached` does, with one
       difference that is the whole point of having it separately: it reads
       the upper database directly instead of going through
       `Store::isValidPath`. The in-memory `pathInfoCache` can already hold a
       positive answer for a path that only the *lower* store has registered,
       because `queryPathInfoUncached` falls back to the lower store and
       returns its info without copying anything up. Once any closure walk has
       seeded that entry, `isValidPath` short-circuits, the copy-up never
       runs, and this store goes on believing a path is registered here when
       no row for it exists. Asking the database is the only way to find out
       what is actually in it. */
    ValidPathInfos toRegister;
    StorePathSet visited;
    std::vector<StorePath> pending{path};

    while (!pending.empty()) {
        auto p = std::move(pending.back());
        pending.pop_back();

        if (!visited.insert(p).second)
            continue;

        /* Already ours. Its references were registered when it was added, so
           there is nothing below it to walk either. */
        if (LocalStore::isValidPathUncached(p))
            continue;

        if (!lowerStore->isValidPath(p))
            continue;

        /* Registering a path this store cannot read would make it assert an
           object that is not there; see `lowerPathVisible`. */
        if (!lowerPathVisible(p))
            continue;

        auto info = lowerStore->queryPathInfo(p);
        for (auto & r : info->references)
            if (r != p)
                pending.push_back(r);
        toRegister.insert_or_assign(p, *info);
    }

    if (!toRegister.empty())
        /* One call, so one transaction: `registerValidPaths` resolves
           references within the batch itself, and a closure member that could
           not be copied up rolls the whole thing back rather than leaving a
           registration pointing at a path this database does not have. */
        LocalStore::registerValidPaths(toRegister);
}

void LocalOverlayStore::registerDrvOutput(const Realisation & info)
{
    /* A realisation's `outputPath` is a foreign key into *this* store's
       `ValidPaths`, and the insert fills it from a subselect, so registering
       one for an output path this database has never heard of writes a NULL
       and dies as `NOT NULL constraint failed: Realisations.outputPath`.
       Nothing else on this path does the copy-up: the lower store's
       realisation names a lower path by definition, and the caller's own
       realisation names one whenever the lower layer gained the output (a
       concurrent build, a substitution, a `nix copy`) before we got here. So
       make the row exist first, for both of them. */
    // First do queryRealisation on lower layer to populate DB
    auto res = lowerStore->queryRealisation(info.id);
    if (res) {
        ensureUpperValidPath(res->outPath);
        LocalStore::registerDrvOutput({*res, info.id});
    }

    ensureUpperValidPath(info.outPath);
    LocalStore::registerDrvOutput(info);
}

bool LocalOverlayStore::lowerPathVisible(const StorePath & path)
{
    /* The lower store's database and this store's store dir can disagree
       about whether a store object is there, and only one of them describes
       what a reader will get. `lowerdir` is allowed to grow while the overlay
       is mounted, but a path looked up before the lower store gained it
       leaves a negative dentry that nothing revalidates, so the merged
       directory goes on answering ENOENT for a directory that is sitting in
       the lower layer. Remounting clears it; the new mount API refuses to
       reconfigure an overlay mount, which is why the tests here force the old
       one, and a caller that cannot remount has no way back.

       Answering "valid" for such a path is the step that turns a mount quirk
       into corruption. `isValidPathUncached` below copies the lower store's
       registration into this store's own database, so from then on this store
       asserts the path on its own account, every later reader is sent to bytes
       that are not there, and the failure lands far from here -- as `path
       '<store dir>/...' does not exist` out of whatever first dumps, copies or
       builds against it, long after the mount is the last thing anyone
       suspects.

       Reporting it invalid is both true of this store and self-healing: the
       caller builds or substitutes the object into the upper layer, where it
       is readable, and the upper layer is what the overlay shows first. */
    if (pathExists(toRealPath(path)))
        return true;

    debug(
        "path '%s' is valid in the lower store but is not visible through the overlay at '%s'; treating it as invalid",
        printStorePath(path),
        PathFmt(config->realStoreDir.get()));
    warnOnce(
        _warnedLowerInvisible,
        "the lower store holds store objects that are not visible through the overlay at %s, so Nix is "
        "treating them as invalid and will rebuild or re-substitute them into the upper layer. This is what "
        "a lower store that gained paths while this overlay was mounted looks like; remount it, or set "
        "`remount-hook`, to pick them up instead.",
        PathFmt(config->realStoreDir.get()));
    return false;
}

void LocalOverlayStore::queryPathInfoUncached(
    const StorePath & path, Callback<std::shared_ptr<const ValidPathInfo>> callback) noexcept
{
    auto callbackPtr = std::make_shared<decltype(callback)>(std::move(callback));

    LocalStore::queryPathInfoUncached(
        path, {[this, path, callbackPtr](std::future<std::shared_ptr<const ValidPathInfo>> fut) {
            try {
                auto info = fut.get();
                if (info)
                    return (*callbackPtr)(std::move(info));
                /* Answer for the layer we serve objects from, not just the
                   lower store's database; see `lowerPathVisible`. Reported as
                   "no such path" rather than as an error because that is what
                   it is from this store: the caller should go make it. */
                if (lowerStore->isValidPath(path) && !lowerPathVisible(path))
                    return (*callbackPtr)(nullptr);
            } catch (...) {
                return callbackPtr->rethrow();
            }
            // If we don't have it, check lower store
            lowerStore->queryPathInfo(path, {[path, callbackPtr](std::future<ref<const ValidPathInfo>> fut) {
                                          try {
                                              (*callbackPtr)(fut.get().get_ptr());
                                          } catch (...) {
                                              return callbackPtr->rethrow();
                                          }
                                      }});
        }});
}

void LocalOverlayStore::queryRealisationUncached(
    const DrvOutput & drvOutput, Callback<std::shared_ptr<const UnkeyedRealisation>> callback) noexcept
{
    auto callbackPtr = std::make_shared<decltype(callback)>(std::move(callback));

    LocalStore::queryRealisationUncached(
        drvOutput, {[this, drvOutput, callbackPtr](std::future<std::shared_ptr<const UnkeyedRealisation>> fut) {
            try {
                auto info = fut.get();
                if (info)
                    return (*callbackPtr)(std::move(info));
            } catch (...) {
                return callbackPtr->rethrow();
            }
            // If we don't have it, check lower store
            lowerStore->queryRealisation(
                drvOutput, {[callbackPtr](std::future<std::shared_ptr<const UnkeyedRealisation>> fut) {
                    try {
                        (*callbackPtr)(fut.get());
                    } catch (...) {
                        return callbackPtr->rethrow();
                    }
                }});
        }});
}

bool LocalOverlayStore::isValidPathUncached(const StorePath & path)
{
    auto res = LocalStore::isValidPathUncached(path);
    if (res)
        return res;
    res = lowerStore->isValidPath(path);
    if (res) {
        /* Do not copy up a registration for something this store cannot
           read; see `lowerPathVisible`. */
        if (!lowerPathVisible(path))
            return false;
        // Get path info from lower store so upper DB genuinely has it.
        auto p = lowerStore->queryPathInfo(path);
        // recur on references, syncing entire closure.
        for (auto & r : p->references)
            if (r != path)
                isValidPath(r);
        LocalStore::registerValidPath(*p);
    }
    return res;
}

void LocalOverlayStore::queryReferrers(const StorePath & path, StorePathSet & referrers)
{
    LocalStore::queryReferrers(path, referrers);
    lowerStore->queryReferrers(path, referrers);
}

void LocalOverlayStore::queryGCReferrers(const StorePath & path, StorePathSet & referrers)
{
    LocalStore::queryReferrers(path, referrers);
}

StorePathSet LocalOverlayStore::queryValidDerivers(const StorePath & path)
{
    auto res = LocalStore::queryValidDerivers(path);
    for (const auto & p : lowerStore->queryValidDerivers(path))
        res.insert(p);
    return res;
}

std::optional<StorePath> LocalOverlayStore::queryPathFromHashPart(const std::string & hashPart)
{
    auto res = LocalStore::queryPathFromHashPart(hashPart);
    if (res)
        return res;
    else
        return lowerStore->queryPathFromHashPart(hashPart);
}

void LocalOverlayStore::registerValidPaths(const ValidPathInfos & infos)
{
    // First, get any from lower store so we merge
    {
        StorePathSet notInUpper;
        for (auto & [p, _] : infos)
            if (!LocalStore::isValidPathUncached(p)) // avoid divergence
                notInUpper.insert(p);
        auto pathsInLower = lowerStore->queryValidPaths(notInUpper);
        ValidPathInfos inLower;
        for (auto & p : pathsInLower)
            inLower.insert_or_assign(p, *lowerStore->queryPathInfo(p));
        LocalStore::registerValidPaths(inLower);
    }
    // Then do original request
    LocalStore::registerValidPaths(infos);
}

void LocalOverlayStore::collectGarbage(const GCOptions & options, GCResults & results)
{
    LocalStore::collectGarbage(options, results);

    remountIfNecessary();
}

void LocalOverlayStore::deleteStorePath(const std::filesystem::path & path, uint64_t & bytesFreed, bool isKnownPath)
{
    if (path.parent_path() != config->realStoreDir.get()) {
        warn("local-overlay: unexpected gc path %s", PathFmt(path));
        return;
    }

    StorePath storePath = {path.filename().string()};
    auto upperPath = config->toUpperPath(storePath);

    if (pathExists(upperPath)) {
        debug("upper exists: %s", PathFmt(path));
        if (lowerStore->isValidPath(storePath)) {
            debug("lower exists: %s", storePath.to_string());
            // Path also exists in lower store.
            // We must delete via upper layer to avoid creating a whiteout.
            deletePath(upperPath, bytesFreed);
            _remountRequired = true;
        } else {
            // Path does not exist in lower store.
            // So we can delete via overlayfs and not need to remount.
            LocalStore::deleteStorePath(path, bytesFreed, isKnownPath);
        }
    }
}

void LocalOverlayStore::optimiseStore()
{
    Activity act(*logger, actOptimiseStore);

    // Note for LocalOverlayStore, queryAllValidPaths only returns paths in upper layer
    auto paths = queryAllValidPaths();

    act.progress(0, paths.size());

    uint64_t done = 0;

    for (auto & path : paths) {
        if (lowerStore->isValidPath(path)) {
            uint64_t bytesFreed = 0;
            // Deduplicate store path
            deleteStorePath(toRealPath(path), bytesFreed, true);
        }
        done++;
        act.progress(done, paths.size());
    }

    remountIfNecessary();
}

LocalStore::VerificationResult LocalOverlayStore::verifyAllValidPaths(RepairFlag repair)
{
    StorePathSet done;

    auto existsInStoreDir = [&](const StorePath & storePath) {
        return pathExists((config->realStoreDir.get() / storePath.to_string()).string());
    };

    bool errors = false;
    StorePathSet validPaths;

    for (auto & i : queryAllValidPaths())
        verifyPath(i, existsInStoreDir, done, validPaths, repair, errors);

    return {
        .errors = errors,
        .validPaths = validPaths,
    };
}

void LocalOverlayStore::remountIfNecessary()
{
    if (!_remountRequired)
        return;

    if (config->remountHook.get().empty()) {
        warn("%s needs remounting, set remount-hook to do this automatically", PathFmt(config->realStoreDir.get()));
    } else {
        runProgram(config->remountHook.get(), false, {config->realStoreDir.get().native()});
    }

    _remountRequired = false;
}

static RegisterStoreImplementation<LocalOverlayStore::Config> regLocalOverlayStore;

} // namespace nix
