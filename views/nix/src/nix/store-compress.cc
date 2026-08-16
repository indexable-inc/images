#include "nix/cmd/command.hh"
#include "nix/main/common-args.hh"
#include "nix/store/store-api.hh"

#ifdef __APPLE__
#  include "nix/store/darwin-compression.hh"
#  include "nix/store/local-fs-store.hh"
#  include "nix/util/util.hh"
#endif

using namespace nix;

struct CmdStoreCompress : StorePathsCommand, MixDryRun
{
    std::string description() override
    {
        return "apply APFS transparent (decmpfs) compression to store paths already on disk";
    }

    std::string doc() override
    {
        return
#include "store-compress.md"
            ;
    }

    void run(ref<Store> store, StorePaths && storePaths) override
    {
#ifdef __APPLE__
        auto localStore = store.dynamic_pointer_cast<LocalFSStore>();
        if (!localStore)
            throw UsageError("'nix store compress' requires a store with a local filesystem, e.g. --store local");

        Activity act(
            *logger,
            lvlInfo,
            actUnknown,
            fmt("%s %d store paths", dryRun ? "measuring" : "compressing", storePaths.size()));

        CompressionStats stats;
        uint64_t done = 0;

        for (auto & path : storePaths) {
            /* Keep the garbage collector from deleting the path out from
               under us; compression never moves or unlinks files, so this is
               the only coordination with other store users that is needed. */
            store->addTempRoot(path);
            if (!store->isValidPath(path))
                continue; /* path was GC'ed */
            {
                Activity actPath(
                    *logger, lvlTalkative, actUnknown, fmt("compressing path '%s'", store->printStorePath(path)));
                compressPathRecursively(localStore->toRealPath(path), stats, dryRun);
            }
            done++;
            act.progress(done, storePaths.size());
        }

        if (dryRun)
            printInfo(
                "would compress %d of %d files, saving %s",
                stats.filesCompressed,
                stats.filesScanned,
                renderSize((int64_t) stats.bytesSaved));
        else
            printInfo(
                "compressed %d of %d files, freeing %s",
                stats.filesCompressed,
                stats.filesScanned,
                renderSize((int64_t) stats.bytesSaved));
        if (stats.filesAlreadyCompressed)
            printInfo("%d files were already compressed", stats.filesAlreadyCompressed);
        if (stats.filesHardLinked)
            printInfo(
                "%d hard-linked (optimised) files totalling %s were left alone",
                stats.filesHardLinked,
                renderSize((int64_t) stats.bytesHardLinked));
        if (stats.filesFailed)
            warn(
                "%d files could not be compressed; compression requires write access to the store "
                "(run as root, or own the store)",
                stats.filesFailed);
#else
        throw UsageError("'nix store compress' uses APFS transparent compression and is only available on macOS");
#endif
    }
};

static auto rCmdStoreCompress = registerCommand2<CmdStoreCompress>({"store", "compress"});
