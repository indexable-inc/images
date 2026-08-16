// TODO: investigate why this is hanging on cygwin
#if !defined(_WIN32) && !defined(__CYGWIN__)

#  include "nix/util/util.hh"
#  include "nix/util/monitor-fd.hh"
#  include "nix/util/signals.hh"

#  include <sys/file.h>
#  include <sys/socket.h>
#  include <chrono>
#  include <gtest/gtest.h>

namespace nix {

// MonitorFdHup calls triggerInterrupt() when it detects a hangup, which sets
// a process-global flag. Clear it after each test so later tests that call
// checkInterrupt() are not poisoned.
class MonitorFdHupTest : public ::testing::Test
{
protected:
    void TearDown() override
    {
        setInterrupted(false);
    }
};

TEST_F(MonitorFdHupTest, shouldNotBlock)
{
    Pipe p;
    p.create();
    {
        // when monitor gets destroyed it should cancel the
        // background thread and do not block
        MonitorFdHup monitor(p.readSide.get());
    }
}

TEST_F(MonitorFdHupTest, shouldExitOnPeerClose)
{
    // Closing the peer end of a socket must end the poll loop promptly.
    int fds[2];
    ASSERT_EQ(socketpair(AF_UNIX, SOCK_STREAM, 0, fds), 0);
    AutoCloseFD our(fds[0]);
    AutoCloseFD peer(fds[1]);

    auto start = std::chrono::steady_clock::now();
    {
        MonitorFdHup monitor(our.get());
        peer.close();
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
        // Destructor joins the thread, which already exited on POLLHUP.
    }
    auto elapsed = std::chrono::steady_clock::now() - start;
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(elapsed).count();
    EXPECT_LT(ms, 1000);
}

#  ifndef __APPLE__
// On Linux, poll() reports POLLNVAL for a closed fd; the monitor must treat
// it as terminal instead of spinning. The macOS path uses kqueue, where a
// closed fd fails registration differently, so the scenario does not apply.
TEST_F(MonitorFdHupTest, shouldExitOnInvalidFd)
{
    Pipe p;
    p.create();
    int fd = p.readSide.get();
    p.readSide.close();

    auto start = std::chrono::steady_clock::now();
    {
        MonitorFdHup monitor(fd);
        std::this_thread::sleep_for(std::chrono::milliseconds(200));
    }
    auto elapsed = std::chrono::steady_clock::now() - start;
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(elapsed).count();
    EXPECT_LT(ms, 1000);
}
#  endif

TEST_F(MonitorFdHupTest, shouldExitOnShutdown)
{
    // shutdown(SHUT_RDWR) on the peer must also read as a hangup.
    int fds[2];
    ASSERT_EQ(socketpair(AF_UNIX, SOCK_STREAM, 0, fds), 0);
    AutoCloseFD our(fds[0]);
    AutoCloseFD peer(fds[1]);

    auto start = std::chrono::steady_clock::now();
    {
        MonitorFdHup monitor(our.get());
        shutdown(peer.get(), SHUT_RDWR);
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
    }
    auto elapsed = std::chrono::steady_clock::now() - start;
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(elapsed).count();
    EXPECT_LT(ms, 1000);
}

} // namespace nix

#endif
