#include "nix/util/thread-pool.hh"

#include <gtest/gtest.h>

#include <atomic>
#include <chrono>
#include <thread>

namespace nix {

namespace {

MakeError(TestError, Error);

} // namespace

TEST(ThreadPool, runsAllWorkItems)
{
    std::atomic<size_t> count{0};
    ThreadPool pool(4);
    for (size_t i = 0; i < 1000; ++i)
        ASSERT_TRUE(pool.tryEnqueue([&count]() { count++; }));
    pool.process();
    ASSERT_EQ(count, 1000);
}

TEST(ThreadPool, processRethrowsWorkItemException)
{
    ThreadPool pool(4);
    pool.enqueue([]() { throw TestError("boom"); });
    ASSERT_THROW(pool.process(), TestError);
}

/* Regression test for the crash class behind queryMissing() core dumps
   (fork commit "join queryMissing's thread pool before its frame dies"):
   a work item throws while the owning thread is still feeding the pool.
   The pool must refuse further items so the feeder can fall through to
   process(), instead of throwing ThreadPoolShutDown through the feeding
   frame while workers still reference it. process() must then rethrow
   the work item's own exception, not a shutdown error. */
TEST(ThreadPool, feederSeesShutdownAfterWorkItemThrows)
{
    ThreadPool pool(2);

    /* Two items, because enqueue only spawns a worker thread once more
       than one item is pending; with a single item nothing runs until
       process(), and the failure could not be observed while feeding. */
    pool.enqueue([]() { throw TestError("boom"); });
    pool.enqueue([]() {});

    /* Keep offering items until the failure is recorded. Bounded so a
       regression fails the test rather than hanging it. */
    bool refused = false;
    for (int i = 0; i < 10000 && !refused; ++i) {
        refused = !pool.tryEnqueue([]() {});
        if (!refused)
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    EXPECT_TRUE(refused);

    EXPECT_THROW(pool.process(), TestError);
}

} // namespace nix
