#include "nix/store/build/build-status.hh"
#include "nix/store/build/derivation-building-goal.hh"
#include "nix/store/build/derivation-goal.hh"
#include "nix/store/build/derivation-trampoline-goal.hh"
#include "nix/store/build/substitution-goal.hh"
#include "nix/store/build/worker.hh"
#include "nix/store/local-fs-store.hh"
#include "nix/store/local-store.hh"
#include "nix/store/globals.hh"
#include "nix/util/experimental-features.hh"
#include "nix/util/file-system.hh"
#include "nix/util/logging.hh"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <cctype>
#include <filesystem>
#include <map>
#include <set>
#include <sstream>
#include <unistd.h>

#ifndef _WIN32
#  include <fcntl.h>
#  include <sys/file.h>
#endif

namespace nix {

/**
 * Per-process client identity, set once per daemon connection in the forked
 * worker. Each worker is its own process, so a process-global is the natural
 * fit (and mirrors how the daemon already stuffs the client pid into argv[1]).
 */
static BuildStatusClientInfo currentClientInfo;

void setBuildStatusClientInfo(BuildStatusClientInfo info)
{
    currentClientInfo = std::move(info);
}

std::filesystem::path buildStatusDir()
{
    return settings.nixStateDir / "status";
}

std::optional<std::filesystem::path> logFileFor(Store & store, const StorePath & drvPath, bool compress)
{
    /* Mirror the formula used by `LogFile` in derivation-building-goal.cc so
       the status file points at the real log. */
    auto baseName = std::string(baseNameOf(store.printStorePath(drvPath)));

    std::filesystem::path logDir;
    if (auto * localStore = dynamic_cast<LocalStore *>(&store))
        logDir = localStore->config->logDir.get();
    else if (dynamic_cast<LocalFSStore *>(&store))
        logDir = settings.getLogFileSettings().nixLogDir;
    else
        return std::nullopt;

    auto dir = logDir / LocalFSStore::drvsLogDir / baseName.substr(0, 2);
    return dir / (baseName.substr(2) + (compress ? ".bz2" : ""));
}

time_t selectBuildNoProgressDeadline(
    const StringSet & requiredSystemFeatures, time_t ordinarySeconds, time_t bigParallelSeconds)
{
    return requiredSystemFeatures.contains("big-parallel") ? bigParallelSeconds : ordinarySeconds;
}

namespace {

#ifdef __linux__
enum class ProcessState {
    other,
    uninterruptibleSleep,
};

ProcessState parseProcessState(char state)
{
    return state == 'D' ? ProcessState::uninterruptibleSleep : ProcessState::other;
}

struct ProcessMetrics
{
    pid_t parentPid;
    uint64_t cpuTicks;
    ProcessState state;
};

struct ProcessTreeMetrics
{
    uint64_t cpuTicks = 0;
    uint64_t ioBytes = 0;
    uint64_t uninterruptibleProcesses = 0;
};

std::map<pid_t, ProcessMetrics> readProcesses()
{
    std::map<pid_t, ProcessMetrics> processes;
    for (auto & entry : std::filesystem::directory_iterator("/proc")) {
        auto name = entry.path().filename().string();
        if (name.empty() || !std::ranges::all_of(name, [](unsigned char c) { return std::isdigit(c); }))
            continue;
        try {
            auto pid = static_cast<pid_t>(std::stol(name));
            auto stat = readFile(entry.path() / "stat");
            auto close = stat.rfind(')');
            if (close == std::string::npos)
                continue;
            std::istringstream fields(stat.substr(close + 2));
            char rawState;
            pid_t parentPid;
            std::string ignored;
            uint64_t userTicks;
            uint64_t systemTicks;
            fields >> rawState >> parentPid;
            for (size_t i = 0; i < 9; ++i)
                fields >> ignored;
            fields >> userTicks >> systemTicks;
            if (fields)
                processes.emplace(pid, ProcessMetrics{parentPid, userTicks + systemTicks, parseProcessState(rawState)});
        } catch (const std::exception &) {
        }
    }
    return processes;
}

ProcessTreeMetrics descendantMetrics(pid_t rootPid)
{
    auto processes = readProcesses();
    std::set<pid_t> selected{rootPid};
    bool changed;
    do {
        changed = false;
        for (auto & [pid, metrics] : processes)
            if (!selected.contains(pid) && selected.contains(metrics.parentPid)) {
                selected.insert(pid);
                changed = true;
            }
    } while (changed);

    ProcessTreeMetrics totals;
    for (auto pid : selected) {
        if (auto process = processes.find(pid); process != processes.end()) {
            totals.cpuTicks += process->second.cpuTicks;
            if (process->second.state == ProcessState::uninterruptibleSleep)
                ++totals.uninterruptibleProcesses;
        }
        try {
            std::istringstream lines(readFile(std::filesystem::path{"/proc"} / std::to_string(pid) / "io"));
            std::string key;
            uint64_t value;
            while (lines >> key >> value)
                if (key == "read_bytes:" || key == "write_bytes:")
                    totals.ioBytes += value;
        } catch (const std::exception &) {
        }
    }
    return totals;
}
#endif

/**
 * The store path a goal is working on, for display in the why-chain.
 */
std::optional<StorePath> goalPath(const Goal & goal)
{
    if (auto * g = dynamic_cast<const DerivationBuildingGoal *>(&goal))
        return g->getDrvPath();
    if (auto * g = dynamic_cast<const DerivationGoal *>(&goal))
        return g->drvPath;
    if (auto * g = dynamic_cast<const DerivationTrampolineGoal *>(&goal))
        return g->drvReq->getBaseStorePath();
    if (auto * g = dynamic_cast<const PathSubstitutionGoal *>(&goal))
        return g->storePath;
    return std::nullopt;
}

/**
 * Walk `waiters` transitively from @p goal until we reach a root goal (one
 * with no waiters, i.e. the top goal the client asked for). The chain is
 * returned root-first: `chain.front()` is the root, `chain.back()` is @p goal.
 *
 * `waiters` are goals waiting *on* this one, so following any waiter ascends
 * towards a root; a goal that nothing waits on is itself a root.
 */
std::vector<StorePath> whyChain(Goal & goal)
{
    std::vector<StorePath> chainLeafFirst;
    std::set<const Goal *> visited;

    Goal * cur = &goal;
    while (cur && !visited.contains(cur)) {
        visited.insert(cur);

        /* Consecutive goals often share a path (the same derivation's
           trampoline, per-output, and building goals all stack up); collapse
           them so the chain reads as one hop per derivation. */
        if (auto p = goalPath(*cur))
            if (chainLeafFirst.empty() || chainLeafFirst.back() != *p)
                chainLeafFirst.push_back(*p);

        /* Ascend to a waiter (a goal waiting on this one). Any waiter leads to
           a root, so following one is enough for the why-chain. A goal with no
           live waiters is a root; stop there. */
        Goal * next = nullptr;
        for (auto & weak : cur->waiters)
            if (auto w = weak.lock()) {
                next = w.get();
                break;
            }
        cur = next;
    }

    std::reverse(chainLeafFirst.begin(), chainLeafFirst.end());
    return chainLeafFirst;
}

nlohmann::json whyJSON(Goal & goal, Store & store, std::string_view cause)
{
    auto chain = whyChain(goal);

    nlohmann::json chainJSON = nlohmann::json::array();
    for (auto & p : chain)
        chainJSON.push_back(store.printStorePath(p));

    nlohmann::json why;
    why["rootDrvPath"] = chain.empty() ? nlohmann::json(nullptr) : nlohmann::json(store.printStorePath(chain.front()));
    why["chain"] = std::move(chainJSON);
    why["cause"] = cause;
    return why;
}

/**
 * Is @p goal a root goal the client asked for directly? A root is a goal that
 * nothing else is waiting on.
 */
bool isTopGoal(Goal & goal)
{
    for (auto & weak : goal.waiters)
        if (weak.lock())
            return false;
    return true;
}

} // namespace

std::unique_ptr<BuildStatus> BuildStatus::forBuild(
    Goal & goal,
    Store & store,
    const StorePath & drvPath,
    std::vector<std::string> outputs,
    std::optional<std::filesystem::path> logFile,
    std::optional<std::string> machineName)
{
    if (!experimentalFeatureSettings.isEnabled(Xp::BuildStatusDir))
        return nullptr;

    auto self = std::unique_ptr<BuildStatus>(new BuildStatus());
    self->logFile = logFile;
    self->drvPath = store.printStorePath(drvPath);
    try {
        nlohmann::json j;
        j["drvPath"] = store.printStorePath(drvPath);
        j["storePath"] = nullptr;
        j["outputs"] = outputs;
        j["type"] = "build";
        j["pid"] = getpid();
        j["clientPid"] =
            currentClientInfo.clientPid ? nlohmann::json(*currentClientInfo.clientPid) : nlohmann::json(nullptr);
        j["startTime"] = time(nullptr);
        j["user"] = currentClientInfo.user ? nlohmann::json(*currentClientInfo.user) : nlohmann::json(nullptr);
        j["uid"] = currentClientInfo.uid ? nlohmann::json(*currentClientInfo.uid) : nlohmann::json(nullptr);
        j["logFile"] = logFile ? nlohmann::json(logFile->string()) : nlohmann::json(nullptr);
        j["machine"] = machineName ? nlohmann::json(*machineName) : nlohmann::json(nullptr);
        j["why"] = whyJSON(goal, store, isTopGoal(goal) ? "requested" : "outputsMissing");
#ifndef _WIN32
        /* Tell readers the file carries a lifetime flock (see `lockFd`). */
        j["livenessLock"] = true;
#endif

        self->path = buildStatusDir() / (std::string(drvPath.to_string()) + "-" + std::to_string(getpid()) + ".json");
        self->write(j.dump());
    } catch (...) {
        /* Observability must never break a build. */
        ignoreExceptionExceptInterrupt();
        return nullptr;
    }
    return self;
}

BuildProgressSnapshot BuildStatus::progressSnapshot() const
{
    BuildProgressSnapshot snapshot;
#ifdef __linux__
    if (builderPid) {
        auto metrics = descendantMetrics(*builderPid);
        snapshot.signals.cpuTicks = metrics.cpuTicks;
        snapshot.signals.ioBytes = metrics.ioBytes;
        snapshot.uninterruptibleProcesses = metrics.uninterruptibleProcesses;
    }
#endif
    if (logFile) {
        std::error_code error;
        snapshot.signals.logBytes = std::filesystem::file_size(*logFile, error);
        if (error)
            snapshot.signals.logBytes = 0;
        error.clear();
        snapshot.signals.logMtime = std::filesystem::last_write_time(*logFile, error);
        if (error)
            snapshot.signals.logMtime = {};
    }
    return snapshot;
}

void BuildStatus::startLiveness(
    std::optional<pid_t> builderPid,
    const StringSet & requiredSystemFeatures,
    time_t ordinarySeconds,
    time_t bigParallelSeconds)
{
    if (ordinarySeconds == 0)
        return;
    if (bigParallelSeconds < ordinarySeconds)
        throw Error(
            "big-parallel-max-no-progress-time (%d) must be at least max-no-progress-time (%d)",
            bigParallelSeconds,
            ordinarySeconds);
    if (!builderPid)
        throw Error("max-no-progress-time requires a local builder process");
#ifndef __linux__
    throw Error("max-no-progress-time requires Linux process activity metrics");
#else
    this->builderPid = builderPid;
    noProgressSeconds = selectBuildNoProgressDeadline(requiredSystemFeatures, ordinarySeconds, bigParallelSeconds);
    lastProgressAt = std::chrono::steady_clock::now();
    lastProgress = progressSnapshot();
#endif
}

std::optional<time_t> BuildStatus::noProgressTimeout(std::chrono::steady_clock::time_point now)
{
    if (noProgressSeconds == 0)
        return std::nullopt;
    auto snapshot = progressSnapshot();
    if (snapshot.hasProgressSince(lastProgress)) {
        lastProgress = snapshot;
        lastProgressAt = now;
        return std::nullopt;
    }
    if (now - lastProgressAt >= std::chrono::seconds(noProgressSeconds)) {
        if (!timeoutReported) {
            printError(
                "derivation '%1%' made no observable progress for %2% seconds; builder-pid=%3%; cpu-ticks=%4%; io-bytes=%5%; log-bytes=%6%; uninterruptible-processes=%7%",
                drvPath,
                noProgressSeconds,
                *builderPid,
                snapshot.signals.cpuTicks,
                snapshot.signals.ioBytes,
                snapshot.signals.logBytes,
                snapshot.uninterruptibleProcesses);
            timeoutReported = true;
        }
        return noProgressSeconds;
    }
    return std::nullopt;
}

std::unique_ptr<BuildStatus> BuildStatus::forSubstitution(Goal & goal, Store & store, const StorePath & storePath)
{
    if (!experimentalFeatureSettings.isEnabled(Xp::BuildStatusDir))
        return nullptr;

    auto self = std::unique_ptr<BuildStatus>(new BuildStatus());
    try {
        nlohmann::json j;
        j["drvPath"] = nullptr;
        j["storePath"] = store.printStorePath(storePath);
        j["outputs"] = nlohmann::json::array();
        j["type"] = "substitution";
        j["pid"] = getpid();
        j["clientPid"] =
            currentClientInfo.clientPid ? nlohmann::json(*currentClientInfo.clientPid) : nlohmann::json(nullptr);
        j["startTime"] = time(nullptr);
        j["user"] = currentClientInfo.user ? nlohmann::json(*currentClientInfo.user) : nlohmann::json(nullptr);
        j["uid"] = currentClientInfo.uid ? nlohmann::json(*currentClientInfo.uid) : nlohmann::json(nullptr);
        j["logFile"] = nullptr;
        j["why"] = whyJSON(goal, store, isTopGoal(goal) ? "requested" : "outputInvalid");
#ifndef _WIN32
        /* Tell readers the file carries a lifetime flock (see `lockFd`). */
        j["livenessLock"] = true;
#endif

        self->path = buildStatusDir() / (std::string(storePath.to_string()) + "-" + std::to_string(getpid()) + ".json");
        self->write(j.dump());
    } catch (...) {
        ignoreExceptionExceptInterrupt();
        return nullptr;
    }
    return self;
}

void BuildStatus::write(std::string_view json)
{
    createDirs(buildStatusDir());
    /* Write to a temp file and rename so a reader never sees a half-written
       file. */
    auto tmp = path;
    tmp += ".tmp";
    writeFile(tmp, json);
#ifndef _WIN32
    /* Lock the entry before the rename makes it visible, so there is no
       window in which a reader can mistake a live entry for a stale one.
       The lock lives on the inode and thus follows the rename. */
    lockFd = AutoCloseFD{open(tmp.c_str(), O_RDWR | O_CLOEXEC)};
    if (!lockFd)
        throw SysError("opening build status file %s", PathFmt(tmp));
    if (flock(lockFd.get(), LOCK_EX | LOCK_NB) != 0)
        throw SysError("locking build status file %s", PathFmt(tmp));
#endif
    std::filesystem::rename(tmp, path);
    active = true;
}

BuildStatus::~BuildStatus()
{
    if (!active)
        return;
    try {
        std::filesystem::remove(path);
    } catch (...) {
        ignoreExceptionInDestructor();
    }
}

} // namespace nix
