#include "nix/fetchers/input-cache.hh"
#include "nix/fetchers/registry.hh"
#include "nix/util/sync.hh"
#include "nix/util/source-path.hh"

namespace nix::fetchers {

InputCache::CachedResult InputCache::getAccessor(
    const Settings & settings, Store & store, const Input & originalInput, UseRegistries useRegistries)
{
    auto fetched = lookup(originalInput);
    Input resolvedInput = originalInput;

    if (!fetched) {
        if (originalInput.isDirect()) {
            auto [accessor, lockedInput] = originalInput.getAccessor(settings, store);
            fetched.emplace(CachedInput{.lockedInput = lockedInput, .accessor = accessor});
        } else {
            if (useRegistries != UseRegistries::No) {
                auto [res, extraAttrs] = lookupInRegistries(settings, store, originalInput, useRegistries);
                resolvedInput = std::move(res);
                fetched = lookup(resolvedInput);
                if (!fetched) {
                    auto [accessor, lockedInput] = resolvedInput.getAccessor(settings, store);
                    fetched.emplace(
                        CachedInput{.lockedInput = lockedInput, .accessor = accessor, .extraAttrs = extraAttrs});
                }
                upsert(resolvedInput, *fetched);
            } else {
                throw Error(
                    "'%s' is an indirect flake reference, but registry lookups are not allowed",
                    originalInput.to_string());
            }
        }
        /* Also cache under the locked input, so a later lookup by the
           locked ref (e.g. relative-input metadata stamping during lock
           computation) reuses this accessor instead of refetching or
           taking the substitution shortcut, which returns a store
           accessor stripped of the fetcher's tree metadata. */
        upsert(fetched->lockedInput, *fetched);
        upsert(originalInput, *fetched);
    }

    debug("got tree '%s' from '%s'", fetched->accessor, fetched->lockedInput.to_string());

    return {fetched->accessor, resolvedInput, fetched->lockedInput, fetched->extraAttrs};
}

struct InputCacheImpl : InputCache
{
    Sync<std::map<Input, CachedInput>> cache_;

    std::optional<CachedInput> lookup(const Input & originalInput) const override
    {
        auto cache(cache_.readLock());
        auto i = cache->find(originalInput);
        if (i == cache->end())
            return std::nullopt;
        debug(
            "mapping '%s' to previously seen input '%s' -> '%s",
            originalInput.to_string(),
            i->first.to_string(),
            i->second.lockedInput.to_string());
        return i->second;
    }

    void upsert(Input key, CachedInput cachedInput) override
    {
        cache_.lock()->insert_or_assign(std::move(key), std::move(cachedInput));
    }

    void clear() override
    {
        cache_.lock()->clear();
    }

    size_t evictUnlocked(const Settings & settings) override
    {
        auto cache(cache_.lock());
        size_t evicted = 0;
        for (auto i = cache->begin(); i != cache->end();) {
            /* Both halves have to be locked. The map is keyed by the original
               input as well as by the locked one, so testing only the key
               keeps an entry a dirty tree reached under a locked-looking
               alias, and testing only the value keeps one whose key is the
               mutable path. */
            if (i->first.isLocked(settings) && i->second.lockedInput.isLocked(settings))
                ++i;
            else {
                i = cache->erase(i);
                evicted++;
            }
        }
        return evicted;
    }
};

ref<InputCache> InputCache::create()
{
    return make_ref<InputCacheImpl>();
}

} // namespace nix::fetchers
