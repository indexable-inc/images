#pragma once
///@file

#include "nix/store/build/worker.hh"
#include "nix/store/store-api.hh"
#include "nix/store/build/goal.hh"
#include "nix/store/build/build-status.hh"
#include "nix/util/muxable-pipe.hh"
#include <coroutine>
#include <future>
#include <source_location>

namespace nix {

struct PathSubstitutionGoal : public Goal
{
    /**
     * The store path that should be realised through a substitute.
     */
    StorePath storePath;

    /**
     * Whether to try to repair a valid path.
     */
    RepairFlag repair;

    /**
     * Pipe for the substituter's standard output.
     */
    MuxablePipe outPipe;

    /**
     * The substituter thread.
     */
    std::thread thr;

    std::unique_ptr<MaintainCount<uint64_t>> maintainExpectedSubstitutions, maintainRunningSubstitutions,
        maintainExpectedNar, maintainExpectedDownload;

    /**
     * Daemon-independent status file for this substitution, present while it
     * is actually running (see the `build-status-dir` experimental feature).
     */
    std::unique_ptr<BuildStatus> buildStatus;

    /**
     * Content address for recomputing store path
     */
    std::optional<ContentAddress> ca;

public:
    PathSubstitutionGoal(
        const StorePath & storePath,
        Worker & worker,
        RepairFlag repair = NoRepair,
        std::optional<ContentAddress> ca = std::nullopt);
    ~PathSubstitutionGoal();

    std::string key() override
    {
        return "a$" + std::string(storePath.name()) + "$" + worker.store.printStorePath(storePath);
    }

    /**
     * The states.
     */
    Co init();
    Co gotInfo();
    Co tryToRun(
        StorePath subPath, nix::ref<Store> sub, std::shared_ptr<const ValidPathInfo> info, bool & substituterFailed);
    Co finished();

    /* Called by destructor, can't be overridden */
    void cleanup() override final;

    JobCategory jobCategory() const override
    {
        return JobCategory::Substitution;
    };
};

} // namespace nix
