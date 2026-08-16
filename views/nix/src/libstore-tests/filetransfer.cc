#include <gtest/gtest.h>

#include <chrono>

#include "nix/store/filetransfer.hh"
#include "nix/util/file-system.hh"

namespace nix {

using namespace std::chrono_literals;

/* The first transfer of a fresh FileTransfer must start promptly. The
   enqueue of that first item and its curl_multi_wakeup() both happen before
   the worker thread has ever reached curl_multi_poll, and libcurl (observed
   with 8.21.0) drains the wakeup eventfd on entering the poll, losing the
   pre-poll wakeup -- so without the sleep re-check in workerThreadMain the
   transfer sits out the worker's whole 10s idle sleep before starting.
   file:// keeps the transfer local and deterministic; the 5s bound gives
   slow CI plenty of headroom while staying far under the 10s hang. */
TEST(FileTransfer, firstTransferStartsPromptly)
{
    auto dir = createTempDir();
    AutoDelete delDir(dir, true);
    auto payload = dir / "payload";
    writeFile(payload, "hello");

    auto ft = makeFileTransfer();
    auto start = std::chrono::steady_clock::now();
    auto res = ft->download(FileTransferRequest(VerbatimURL("file://" + payload.string())));
    auto elapsed = std::chrono::steady_clock::now() - start;

    EXPECT_EQ(res.data, "hello");
    EXPECT_LT(elapsed, 5s);
}

/* A half-closed peer can never finish delivering the body, so the transfer
   must fail immediately -- even below the stall deadline (index#3559). */
TEST(ClassifyPausedTransfer, HalfClosedFailsImmediately)
{
    EXPECT_EQ(classifyPausedTransfer(true, 0s, 300s), PausedTransferVerdict::failHalfClosed);
}

/* ... and even when the silent-stall deadline is disabled. */
TEST(ClassifyPausedTransfer, HalfClosedFailsWithStallDisabled)
{
    EXPECT_EQ(classifyPausedTransfer(true, 0s, 0s), PausedTransferVerdict::failHalfClosed);
}

/* A live connection paused below the deadline is healthy backpressure. */
TEST(ClassifyPausedTransfer, LivePauseBelowDeadlineKeepsWaiting)
{
    EXPECT_EQ(classifyPausedTransfer(false, 5s, 300s), PausedTransferVerdict::keepWaiting);
}

/* A silent (not half-closed) transfer paused at or past the deadline fails. */
TEST(ClassifyPausedTransfer, SilentStallPastDeadlineFails)
{
    EXPECT_EQ(classifyPausedTransfer(false, 300s, 300s), PausedTransferVerdict::failStalled);
    EXPECT_EQ(classifyPausedTransfer(false, 301s, 300s), PausedTransferVerdict::failStalled);
}

/* stalled-download-timeout == 0 disables the silent-stall deadline. */
TEST(ClassifyPausedTransfer, StallDisabledNeverStalls)
{
    EXPECT_EQ(classifyPausedTransfer(false, 100000s, 0s), PausedTransferVerdict::keepWaiting);
}

} // namespace nix
