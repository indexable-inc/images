#include "invocation-record.hh"

#include "nix/expr/counter.hh"
#include "nix/expr/eval.hh"
#include "nix/store/globals.hh"
#include "nix/util/config-global.hh"
#include "nix/util/environment-variables.hh"
#include "nix/util/experimental-features.hh"
#include "nix/util/file-system.hh"
#include "nix/util/logging.hh"
#include "nix/util/users.hh"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <chrono>
#include <random>
#include <vector>

#ifndef _WIN32
#  include <unistd.h>
#endif

namespace nix {

namespace {

struct InvocationRecordSettings : Config
{
    Setting<uint64_t> keep{
        this,
        100,
        "keep-invocation-records",
        R"(
          How many invocation records to keep. The oldest are deleted at the
          start of each recorded invocation. Set to `0` to keep every record,
          in which case something else has to delete them.
        )"};
};

InvocationRecordSettings invocationRecordSettings;

GlobalConfig::Register rInvocationRecordSettings(&invocationRecordSettings);

/**
 * Set in the environment of every child process, so a build hook or a
 * `__build-remote` self-invocation joins this record instead of minting its
 * own.
 */
constexpr auto envName = "NIX_INVOCATION_ID";

std::optional<std::string> currentId;
std::filesystem::path currentDir;
Strings currentArgv;

/**
 * When this process started, near enough. Taken at dynamic initialisation
 * rather than when the record is minted, because minting happens after the
 * command line is parsed and the config is loaded, and a wall clock that
 * excludes those is not the wall clock anyone means.
 */
const auto processStartedAt = std::chrono::system_clock::now();

int64_t microsSince(std::chrono::system_clock::time_point t)
{
    return std::chrono::duration_cast<std::chrono::microseconds>(t.time_since_epoch()).count();
}

/**
 * A time-ordered random id: 12 hex digits of Unix milliseconds, then 12 hex
 * digits from the system entropy source.
 *
 * Time-ordered so the record directory sorts oldest first and retention is a
 * sort of the names rather than a stat of every entry. Random rather than
 * derived from the build's content, because two runs of the same `nix build`
 * on the same lock are different events with different durations and
 * different cache hits: a content hash would collide exactly the two things
 * this exists to tell apart. Bazel and Buck2 both use a random UUID here.
 */
std::string mintId()
{
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::system_clock::now().time_since_epoch())
                  .count();
    std::random_device rd;
    uint64_t entropy = (uint64_t(rd()) << 32) | rd();
    return fmt("%012x%012x", uint64_t(ms) & 0xffffffffffffULL, entropy & 0xffffffffffffULL);
}

std::filesystem::path recordsDir()
{
    return getStateDir() / "invocations";
}

/**
 * Delete all but the newest `keep-invocation-records` records. Ids sort by
 * time, so this is a sort of the directory names.
 */
void prune()
{
    auto keep = invocationRecordSettings.keep.get();
    if (keep == 0)
        return;
    std::vector<std::filesystem::path> entries;
    std::error_code ec;
    for (auto & entry : std::filesystem::directory_iterator{recordsDir(), ec})
        entries.push_back(entry.path());
    if (ec || entries.size() <= keep)
        return;
    std::sort(entries.begin(), entries.end());
    for (size_t i = 0; i + keep < entries.size(); ++i)
        std::filesystem::remove_all(entries[i], ec);
}

} // namespace

void invocationRecord::start(const Strings & argv, bool suppress)
{
    if (suppress || !experimentalFeatureSettings.isEnabled(Xp::InvocationRecords))
        return;
    /* A build hook or `__build-remote` self-invocation is part of this
       invocation, not a new one. */
    if (getEnv(envName))
        return;

    try {
        auto id = mintId();
        auto dir = recordsDir() / id;
        createDirs(dir);
        prune();

        /* Tee the internal-json event stream into the record. Timestamps are
           on: without them a reader can order events but cannot time them,
           which is the whole point of reading the record afterwards. */
        std::vector<std::unique_ptr<Logger>> extra;
        extra.push_back(makeJSONLogger(dir / "events.jsonl", false, true));
        logger = makeTeeLogger(std::move(logger), std::move(extra));

        /* The evaluator's counters are compiled to no-ops unless this is set
           before evaluation starts; `~EvalCommand` then writes them out. */
        Counter::enabled = true;
        evalStatsPath = dir / "eval-stats.json";

        setEnv(envName, id.c_str());

        currentId = id;
        currentDir = dir;
        currentArgv = argv;
    } catch (...) {
        /* Observability must never break a command. */
        ignoreExceptionExceptInterrupt();
        currentId.reset();
    }
}

void invocationRecord::finish(int exitStatus)
{
    if (!currentId)
        return;
    try {
        auto now = std::chrono::system_clock::now();
        nlohmann::json j;
        j["invocationId"] = *currentId;
        j["nixVersion"] = nixVersion;
        j["argv"] = currentArgv;
        j["commandLine"] = concatStringsSep(" ", currentArgv);
        j["cwd"] = std::filesystem::current_path().string();
        j["pid"] = getpid();
        j["startedAtUs"] = microsSince(processStartedAt);
        j["endedAtUs"] = microsSince(now);
        j["exitStatus"] = exitStatus;
        j["wallSeconds"] = std::chrono::duration<double>(now - processStartedAt).count();
        writeFile(currentDir / "meta.json", j.dump());

        /* Notice level goes to stderr, so `--json` output on stdout stays
           parseable, and `--quiet` still silences it. */
        logger->log(lvlNotice, fmt("invocation %s", *currentId));
    } catch (...) {
        ignoreExceptionExceptInterrupt();
    }
    currentId.reset();
}

} // namespace nix
