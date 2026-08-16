#include <gtest/gtest.h>

#include "nix/store/build/build-status.hh"

namespace nix {

TEST(BuildProgressSnapshotTest, DStateWithoutProgressIsDiagnosticOnly)
{
    BuildProgressSnapshot previous;
    auto current = previous;
    current.uninterruptibleProcesses = 1;

    EXPECT_FALSE(current.hasProgressSince(previous));
}

TEST(BuildProgressSnapshotTest, DStateWithCpuProgressResetsDeadline)
{
    BuildProgressSnapshot previous;
    auto current = previous;
    current.uninterruptibleProcesses = 1;
    current.signals.cpuTicks = 1;

    EXPECT_TRUE(current.hasProgressSince(previous));
}

TEST(BuildProgressSnapshotTest, BigParallelUsesLongerDeadline)
{
    EXPECT_EQ(selectBuildNoProgressDeadline(StringSet{}, 60, 300), 60);
    EXPECT_EQ(selectBuildNoProgressDeadline(StringSet{"big-parallel"}, 60, 300), 300);
}

} // namespace nix
