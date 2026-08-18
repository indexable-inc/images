#include "nix/fetchers/fetch-to-store.hh"
#include "nix/fetchers/fetchers.hh"
#include "nix/fetchers/fetch-settings.hh"
#include "nix/util/environment-variables.hh"
#include "nix/util/configuration.hh"

namespace nix {

fetchers::Cache::Key
makeSourcePathToHashCacheKey(std::string_view fingerprint, ContentAddressMethod method, const CanonPath & path)
{
    return fetchers::Cache::Key{
        "sourcePathToHash",
        {{"fingerprint", std::string(fingerprint)}, {"method", std::string{method.render()}}, {"path", path.abs()}}};
}

StorePath fetchToStore(
    const fetchers::Settings & settings,
    Store & store,
    const SourcePath & path,
    FetchMode mode,
    std::string_view name,
    ContentAddressMethod method,
    PathFilter * filter,
    RepairFlag repair)
{
    return fetchToStore2(settings, store, path, mode, name, method, filter, repair).first;
}

std::pair<StorePath, Hash> fetchToStore2(
    const fetchers::Settings & settings,
    Store & store,
    const SourcePath & path,
    FetchMode mode,
    std::string_view name,
    ContentAddressMethod method,
    PathFilter * filter,
    RepairFlag repair)
{
    std::optional<fetchers::Cache::Key> cacheKey;

    auto [subpath, fingerprint] = filter ? std::pair<CanonPath, std::optional<std::string>>{path.path, std::nullopt}
                                         : path.accessor->getFingerprint(path.path);

    /* An accessor reading out of a content-addressed object store (the jj
       workdir fetcher) knows its root's tree id a priori: the VCS
       maintained it incrementally, Merkle-fashion, while snapshotting. When
       the id's family is one nix also ingests, that id IS the content
       address, so the store path follows from it with zero file reads --
       where the NAR method's flat hash has to re-read the whole tree on
       every content change. A family nix does not ingest is not an address
       here and falls through to reading the tree. */
    std::optional<Hash> knownHash;
    if (method == ContentAddressMethod::Raw::Git && !filter && path.path.isRoot() && path.accessor->knownTreeRoot
        && path.accessor->knownTreeRoot->family == KnownTreeRoot::Family::Git
        && experimentalFeatureSettings.isEnabled(Xp::GitHashing))
        knownHash = path.accessor->knownTreeRoot->id;

    std::optional<Hash> trustedHash;
    bool trustedFromCache = false;

    if (fingerprint) {
        cacheKey = makeSourcePathToHashCacheKey(*fingerprint, method, subpath);
        if (auto res = settings.getCache()->lookup(*cacheKey)) {
            trustedHash = Hash::parseSRI(fetchers::getStrAttr(*res, "hash"));
            trustedFromCache = true;
        }
    } else if (!knownHash) {
        static auto barf = getEnv("_NIX_TEST_BARF_ON_UNCACHEABLE").value_or("") == "1";
        if (barf && !filter)
            throw Error("source path '%s' is uncacheable (filter=%d)", path, (bool) filter);
        // FIXME: could still provide in-memory caching keyed on `SourcePath`.
        debug("source path '%s' is uncacheable", path);
    }

    if (!trustedHash)
        trustedHash = knownHash;

    if (trustedHash) {
        auto storePath =
            store.makeFixedOutputPathFromCA(name, ContentAddressWithReferences::fromParts(method, *trustedHash, {}));

        /* Add a temproot before the call to isValidPath to prevent accidental GC in case the
           input is cached. Note that this must be done before to avoid races. */
        if (mode != FetchMode::DryRun)
            store.addTempRoot(storePath);

        if (mode == FetchMode::DryRun || store.isValidPath(storePath)) {
            /* Seed the cache when the hash came from the accessor rather
               than the cache, so a later process without the announcing
               fetcher still hits. */
            if (cacheKey && !trustedFromCache)
                settings.getCache()->upsert(*cacheKey, {{"hash", trustedHash->to_string(HashFormat::SRI, true)}});
            debug(
                "source path '%s' %s in '%s' (hash '%s')",
                path,
                trustedFromCache ? "cache hit" : "resolved by announced git tree hash",
                store.printStorePath(storePath),
                trustedHash->to_string(HashFormat::SRI, true));
            return {storePath, *trustedHash};
        }
        debug("source path '%s' not in store", path);
    }

    Activity act(
        *logger,
        lvlChatty,
        actUnknown,
        fmt(mode == FetchMode::DryRun ? "hashing '%s'" : "copying '%s' to the store", path));

    auto filter2 = filter ? *filter : defaultPathFilter;

    /* The walk must agree with an announced hash's algorithm (git object
       hashes are SHA-1 in every repo jj creates today). */
    auto hashAlgo = knownHash ? knownHash->algo : HashAlgorithm::SHA256;

    auto [storePath, hash] =
        mode == FetchMode::DryRun
            ? [&]() {
                  auto [storePath, hash] =
                      store.computeStorePath(name, path, method, hashAlgo, {}, filter2);
                  debug(
                      "hashed '%s' to '%s' (hash '%s')",
                      path,
                      store.printStorePath(storePath),
                      hash.to_string(HashFormat::SRI, true));
                  return std::make_pair(storePath, hash);
              }()
            : [&]() {
                  // FIXME: ideally addToStore() would return the hash
                  // right away (like computeStorePath()).
                  auto storePath = store.addToStore(name, path, method, hashAlgo, {}, filter2, repair);
                  auto info = store.queryPathInfo(storePath);
                  assert(info->references.empty());
                  auto hash = method == ContentAddressMethod::Raw::NixArchive ? info->narHash : ({
                      if (!info->ca || info->ca->method != method)
                          throw Error("path '%s' lacks a CA field", store.printStorePath(storePath));
                      info->ca->hash;
                  });
                  debug(
                      "copied '%s' to '%s' (hash '%s')",
                      path,
                      store.printStorePath(storePath),
                      hash.to_string(HashFormat::SRI, true));
                  return std::make_pair(storePath, hash);
              }();

    if (knownHash && hash != *knownHash)
        warn(
            "accessor for '%s' announced git tree hash '%s' but its content hashed to '%s'; "
            "a dry-run mount derived from the announced hash will fail with a store path mismatch",
            path,
            knownHash->to_string(HashFormat::SRI, true),
            hash.to_string(HashFormat::SRI, true));

    if (cacheKey)
        settings.getCache()->upsert(*cacheKey, {{"hash", hash.to_string(HashFormat::SRI, true)}});

    return {storePath, hash};
}

} // namespace nix
