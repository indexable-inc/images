#include "nix/util/processes.hh"
#include "nix/util/error.hh"

#include <gtest/gtest.h>

namespace nix {

/* ----------------------------------------------------------------------------
 * statusOk
 * --------------------------------------------------------------------------*/

TEST(statusOk, zeroIsOk)
{
    ASSERT_EQ(statusOk(0), true);
    ASSERT_EQ(statusOk(1), false);
}

/* ----------------------------------------------------------------------------
 * Pid::wait / Pid::kill on an unset Pid
 *
 * A default-constructed Pid (or one that has already been waited on, which
 * resets it to the same state) is a state a caller can be holding; it used
 * to be an assert(), which aborts the whole process rather than raising
 * something a caller can handle. This was reached in production on a
 * sandbox-setup failure with no builder child yet
 * (derivation-builder.cc's processSandboxSetupMessages), and is reachable
 * again via HttpsBinaryCacheStoreTest::TearDown on a platform where SetUp
 * GTEST_SKIPs before ever assigning its Pid member.
 * --------------------------------------------------------------------------*/

TEST(Pid, waitOnUnsetPidThrowsRatherThanAborts)
{
    Pid pid;
    ASSERT_THROW(pid.wait(), Error);
}

TEST(Pid, killOnUnsetPidThrowsRatherThanAborts)
{
    Pid pid;
    ASSERT_THROW(pid.kill(), Error);
}

} // namespace nix
