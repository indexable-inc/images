#pragma once
///@file

#include "nix/store/path.hh"
#include "nix/store/build/goal.hh"
#include "nix/util/file-descriptor.hh"

#include <chrono>
#include <optional>
#include <string>
#include <vector>

namespace nix {

class Store;

/** Signals that decide whether a running builder has made progress. */
struct BuildProgressSignals
{
    uint64_t cpuTicks = 0;
    uint64_t ioBytes = 0;
    uintmax_t logBytes = 0;
    std::filesystem::file_time_type logMtime{};

    bool operator==(const BuildProgressSignals &) const = default;
};

/**
 * Builder observations at one instant. Kernel wait state is diagnostic only
 * and cannot reset or trigger the no-progress deadline.
 */
struct BuildProgressSnapshot
{
    BuildProgressSignals signals;
    uint64_t uninterruptibleProcesses = 0;

    bool hasProgressSince(const BuildProgressSnapshot & previous) const
    {
        return signals != previous.signals;
    }
};

time_t selectBuildNoProgressDeadline(
    const StringSet & requiredSystemFeatures, time_t ordinarySeconds, time_t bigParallelSeconds);

/**
 * Identity of the client that requested the work a goal is doing.
 *
 * The daemon forks one worker process per client connection, so this is
 * recorded once per connection as a process-global (see @ref
 * setBuildStatusClientInfo) and read back by every goal running in that
 * worker.
 */
struct BuildStatusClientInfo
{
    std::optional<pid_t> clientPid;
    std::optional<uid_t> uid;
    std::optional<std::string> user;
};

/**
 * Record the identity of the client for the current process. Called once per
 * connection in the forked daemon worker. Safe to leave unset (e.g. for a
 * local store with no daemon), in which case the recorded client pid, uid,
 * and user are null.
 */
void setBuildStatusClientInfo(BuildStatusClientInfo info);

/**
 * The directory into which status files are written, i.e.
 * `<store-state-dir>/status`. Derived from `settings.nixStateDir`, so it
 * honors `NIX_STATE_DIR`.
 */
std::filesystem::path buildStatusDir();

/**
 * Compute the path of the build log file for @p drvPath, using the exact same
 * formula as the build log writer (see `LogFile` in
 * derivation-building-goal.cc). Returns `std::nullopt` if build logs are not
 * being kept.
 */
std::optional<std::filesystem::path> logFileFor(Store & store, const StorePath & drvPath, bool compress);

/**
 * RAII writer for a single goal's status file under @ref buildStatusDir.
 *
 * The constructor writes one JSON file atomically (write-to-temp + rename)
 * describing the goal that is now doing real work; the destructor removes it.
 * Instantiate a `BuildStatus` at the point a goal starts a build or
 * substitution, and store it as a goal member so RAII removes the file when
 * the goal finishes or is destroyed.
 *
 * Writing is gated behind the `build-status-dir` experimental feature: if the
 * feature is disabled the writer is a no-op.
 */
class BuildStatus
{
    std::filesystem::path path;
    bool active = false;

    /**
     * Held (`flock`ed) for the lifetime of this object so readers can tell
     * a live writer from a corpse: the kernel releases the lock on any kind
     * of process death -- SIGKILL, a crash, or surviving only as a zombie --
     * whereas the recorded pid keeps passing `kill(pid, 0)` for zombies.
     */
    AutoCloseFD lockFd;

    std::optional<pid_t> builderPid;
    std::string drvPath;
    std::optional<std::filesystem::path> logFile;
    time_t noProgressSeconds = 0;
    std::chrono::steady_clock::time_point lastProgressAt;
    BuildProgressSnapshot lastProgress;
    bool timeoutReported = false;

    BuildProgressSnapshot progressSnapshot() const;

public:
    /**
     * Write the status file for a build goal.
     *
     * @param goal The goal doing the work; its `waiters` are walked to
     * compute the why-chain up to the root goal the client requested.
     * @param drvPath The derivation being built.
     * @param outputs The wanted output names.
     * @param logFile The on-disk build log path, if any.
     * @param machineName The remote builder running this build, or nullopt
     * when it runs here. Recorded because a reader attaching to a running
     * build cannot otherwise tell where it is: the log stream a remote build
     * relays back reports no machine, so this file is the only place the
     * answer survives.
     */
    static std::unique_ptr<BuildStatus> forBuild(
        Goal & goal,
        Store & store,
        const StorePath & drvPath,
        std::vector<std::string> outputs,
        std::optional<std::filesystem::path> logFile,
        std::optional<std::string> machineName = {});

    /**
     * Write the status file for a substitution goal.
     */
    static std::unique_ptr<BuildStatus> forSubstitution(Goal & goal, Store & store, const StorePath & storePath);

    void startLiveness(
        std::optional<pid_t> builderPid,
        const StringSet & requiredSystemFeatures,
        time_t ordinarySeconds,
        time_t bigParallelSeconds);

    std::optional<time_t> noProgressTimeout(std::chrono::steady_clock::time_point now);

    BuildStatus(const BuildStatus &) = delete;
    BuildStatus & operator=(const BuildStatus &) = delete;

    ~BuildStatus();

private:
    BuildStatus() = default;

    void write(std::string_view json);
};

} // namespace nix
