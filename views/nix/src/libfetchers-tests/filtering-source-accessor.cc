#include <gtest/gtest.h>

#include <thread>
#include <vector>

#include <boost/unordered/unordered_flat_set.hpp>

#include "nix/fetchers/filtering-source-accessor.hh"
#include "nix/util/memory-source-accessor.hh"

namespace nix {

/**
 * `AllowListSourceAccessor::allowPrefix` used to insert into an unsynchronised
 * `std::set`. Under the parallel evaluator two threads reaching
 * `EvalState::allowPath` at once corrupted the red-black tree and dereferenced
 * null inside `std::__tree_balance_after_insert`, which showed up as a SIGSEGV
 * in one of nine twelve-host evaluations. Hammering the insert path directly
 * makes that deterministic instead of a one-in-nine flake.
 */
TEST(AllowListSourceAccessor, concurrentAllowPrefixDoesNotCorruptTheSet)
{
    constexpr size_t nThreads = 16;
    constexpr size_t nPerThread = 4000;

    auto accessor =
        AllowListSourceAccessor::create(make_ref<MemorySourceAccessor>(), {}, {}, [](const CanonPath & path) {
            return RestrictedPathError("access to '%s' is forbidden", path);
        });

    std::vector<std::thread> threads;
    for (size_t t = 0; t < nThreads; ++t)
        threads.emplace_back([&accessor, t]() {
            for (size_t i = 0; i < nPerThread; ++i) {
                accessor->allowPrefix(CanonPath("/t" + std::to_string(t) + "/p" + std::to_string(i)));
                /* Readers share the tree with the writers, so interleave them:
                   a reader walking a node another thread is rebalancing is the
                   other half of the original crash. */
                (void) accessor->isAllowed(CanonPath("/t0/p0/somewhere/deeper"));
            }
        });
    for (auto & t : threads)
        t.join();

    /* A crash is the loud failure. This is the quiet one: an unsynchronised
       insert can also simply lose a prefix, which would silently grant or deny
       the wrong path rather than crash. */
    for (size_t t = 0; t < nThreads; ++t)
        for (size_t i = 0; i < nPerThread; ++i)
            ASSERT_TRUE(accessor->isAllowed(CanonPath("/t" + std::to_string(t) + "/p" + std::to_string(i))))
                << "prefix /t" << t << "/p" << i << " was lost";
}

} // namespace nix
