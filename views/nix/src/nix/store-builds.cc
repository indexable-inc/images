#include "nix/cmd/command.hh"
#include "nix/main/common-args.hh"
#include "nix/store/globals.hh"
#include "nix/util/experimental-features.hh"
#include "nix/util/file-descriptor.hh"
#include "nix/util/file-system.hh"
#include "nix/util/logging.hh"

#include <nlohmann/json.hpp>

#include <csignal>

#ifndef _WIN32
#  include <fcntl.h>
#  include <sys/file.h>
#endif
#ifdef __APPLE__
#  include <sys/sysctl.h>
#endif

using namespace nix;

/**
 * The status directory, i.e. `<store-state-dir>/status`. Kept in sync with
 * `buildStatusDir()` in libstore, but computed here from `settings.nixStateDir`
 * so this command never has to link against or open a store: it honors
 * `NIX_STATE_DIR` just like the writer.
 */
static std::filesystem::path statusDir()
{
    return settings.nixStateDir / "status";
}

/**
 * Is the process @p pid still alive? A status file whose writer has died
 * (e.g. after a crash) is stale and should be ignored/removed.
 */
static bool pidAlive(pid_t pid)
{
    if (pid <= 0)
        return false;
    if (::kill(pid, 0) == 0)
        return true;
    return errno != ESRCH;
}

/**
 * Is @p pid a zombie? `kill(pid, 0)` still succeeds for a zombie, so
 * `pidAlive` alone keeps reporting builds "in flight" for workers that died
 * unreaped -- the observed incident shape: 33 phantom entries owned by three
 * zombie daemon workers under an unreapable parent, misleading every
 * observer for 10.5 hours.
 */
static bool pidZombie(pid_t pid)
{
#ifdef __APPLE__
    struct kinfo_proc info;
    size_t len = sizeof(info);
    int mib[4] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, pid};
    if (sysctl(mib, 4, &info, &len, nullptr, 0) != 0 || len < sizeof(info))
        return false;
    return info.kp_proc.p_stat == SZOMB;
#elif defined(__linux__)
    try {
        auto stat = readFile(std::filesystem::path{"/proc"} / std::to_string(pid) / "stat");
        /* The state field follows the parenthesized, possibly
           space-containing command name. */
        auto close = stat.rfind(')');
        if (close == std::string::npos || close + 2 >= stat.size())
            return false;
        return stat[close + 2] == 'Z';
    } catch (...) {
        return false;
    }
#else
    return false;
#endif
}

/**
 * Is the writer of the status file @p p still alive?
 *
 * Writers that set `livenessLock` hold an `flock` on the file for their
 * lifetime; the kernel releases it on any kind of death (SIGKILL, crash,
 * zombie), so an acquirable lock proves the writer is gone. Older entries
 * fall back to probing the recorded pid, treating zombies as dead.
 */
static bool writerAlive(const std::filesystem::path & p, const nlohmann::json & j)
{
#ifndef _WIN32
    if (j.value("livenessLock", false)) {
        AutoCloseFD fd{open(p.c_str(), O_RDONLY | O_CLOEXEC)};
        if (!fd)
            return false;
        /* Only a successful acquisition proves the writer gone; any flock
           failure (EWOULDBLOCK from the live writer's lock, or a freak
           EINTR/ENOLCK) keeps the entry. A genuinely stale file is pruned
           on the next read. */
        return flock(fd.get(), LOCK_SH | LOCK_NB) != 0;
    }
#endif
    auto pidIt = j.find("pid");
    if (pidIt == j.end() || !pidIt->is_number())
        return true; /* no way to probe; keep the entry */
    auto pid = pidIt->get<pid_t>();
    return pidAlive(pid) && !pidZombie(pid);
}

struct CmdStoreBuilds : Command, MixJSON
{
    std::string description() override
    {
        return "show the builds and substitutions currently in progress";
    }

    std::string doc() override
    {
        return
#include "store-builds.md"
            ;
    }

    Category category() override
    {
        return catUtility;
    }

    std::optional<ExperimentalFeature> experimentalFeature() override
    {
        return Xp::BuildStatusDir;
    }

    void run() override
    {
        /* The `experimentalFeature()` override advertises the gate (e.g. in
           `nix __dump-cli`), but the multi-command dispatcher does not enforce
           a subcommand's feature automatically, so require it here too. */
        experimentalFeatureSettings.require(Xp::BuildStatusDir);

        auto dir = statusDir();

        std::vector<nlohmann::json> entries;

        if (std::filesystem::exists(dir)) {
            for (auto & entry : std::filesystem::directory_iterator{dir}) {
                auto & p = entry.path();
                if (p.extension() != ".json")
                    continue;

                nlohmann::json j;
                try {
                    j = nlohmann::json::parse(readFile(p.string()));
                } catch (...) {
                    /* A half-written or corrupt file; skip it. */
                    continue;
                }

                /* Drop entries whose writer is no longer alive. */
                if (!writerAlive(p, j)) {
                    try {
                        std::filesystem::remove(p);
                    } catch (...) {
                    }
                    continue;
                }

                entries.push_back(std::move(j));
            }
        }

        if (json) {
            printJSON(nlohmann::json(entries));
            return;
        }

        if (entries.empty()) {
            notice("No builds or substitutions are currently in progress.");
            return;
        }

        for (auto & j : entries) {
            auto type = j.value("type", "?");
            auto pid = j.value("pid", 0);
            auto user = j.contains("user") && !j["user"].is_null() ? j["user"].get<std::string>() : "<unknown>";
            if (type == "substitution")
                logger->cout("%s  substituting %s  (pid %d, user %s)", type, j.value("storePath", "?"), pid, user);
            else
                logger->cout("%s  building %s  (pid %d, user %s)", type, j.value("drvPath", "?"), pid, user);
            if (j.contains("why") && j["why"].contains("chain")) {
                auto & chain = j["why"]["chain"];
                if (chain.is_array() && !chain.empty())
                    logger->cout(
                        "    because %s wants it (%s)", chain.front().get<std::string>(), j["why"].value("cause", "?"));
            }
        }
    }
};

static auto rCmdStoreBuilds = registerCommand2<CmdStoreBuilds>({"store", "builds"});
