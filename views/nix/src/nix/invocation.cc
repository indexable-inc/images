#include "nix/cmd/command.hh"
#include "nix/main/common-args.hh"
#include "nix/util/experimental-features.hh"
#include "nix/util/file-system.hh"
#include "nix/util/logging.hh"
#include "nix/util/users.hh"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <fstream>
#include <map>

using namespace nix;

namespace {

/**
 * Kept in sync with the writer in invocation-record.cc. Both derive it from
 * `getStateDir()`, so both honour `NIX_STATE_HOME` and `XDG_STATE_HOME`.
 */
std::filesystem::path recordsDir()
{
    return getStateDir() / "invocations";
}

/**
 * `Activity` type codes from `nix/util/logging.hh`. The event stream carries
 * the number, not the name, so the reader has to name them.
 */
constexpr int actBuild = 105;
constexpr int actSubstitute = 108;

nlohmann::json readJSONFile(const std::filesystem::path & p)
{
    if (!std::filesystem::exists(p))
        return nlohmann::json::object();
    try {
        return nlohmann::json::parse(readFile(p.string()));
    } catch (...) {
        return nlohmann::json::object();
    }
}

/**
 * Resolve a possibly abbreviated invocation id, or `last`, to a record
 * directory. Ids sort by time, so `last` is the greatest name.
 */
std::filesystem::path resolve(const std::string & idOrLast)
{
    std::vector<std::string> names;
    std::error_code ec;
    for (auto & entry : std::filesystem::directory_iterator{recordsDir(), ec})
        names.push_back(entry.path().filename().string());
    std::sort(names.begin(), names.end());

    if (names.empty())
        throw Error("no invocation records under %s", PathFmt(recordsDir()));

    if (idOrLast == "last")
        return recordsDir() / names.back();

    std::vector<std::string> matches;
    for (auto & n : names)
        if (n.starts_with(idOrLast))
            matches.push_back(n);
    if (matches.empty())
        throw Error("no invocation record matches '%s'", idOrLast);
    if (matches.size() > 1)
        throw Error("'%s' matches %d invocation records; use more characters", idOrLast, matches.size());
    return recordsDir() / matches.front();
}

/**
 * One derivation build or substitution inside an invocation.
 */
struct Span
{
    std::string kind;
    std::string path;
    /** The remote builder that ran it, empty for a local build. */
    std::string machine;
    int64_t startUs = 0;
    std::optional<int64_t> endUs;

    double seconds(int64_t fallbackEnd) const
    {
        return double(endUs.value_or(fallbackEnd) - startUs) / 1e6;
    }
};

/**
 * An already-kept span of the same kind and path that this one starts inside,
 * if there is one.
 */
Span * enclosing(std::vector<Span> & kept, const Span & s)
{
    for (auto & k : kept)
        if (k.kind == s.kind && k.path == s.path && k.startUs <= s.startUs && (!k.endUs || *k.endUs >= s.startUs))
            return &k;
    return nullptr;
}

/**
 * Fold the event stream into one span per build and substitution.
 *
 * Activity ids are unique within an invocation, so a `start` and the `stop`
 * carrying the same id are the two ends of one span. A span with no `stop`
 * belongs to work that was still running when the command died; it is
 * reported, with its end clamped to the end of the record.
 */
std::vector<Span> spansOf(const std::filesystem::path & events)
{
    std::map<uint64_t, Span> open;
    std::vector<Span> done;

    std::ifstream in(events);
    std::string line;
    while (std::getline(in, line)) {
        nlohmann::json j;
        try {
            j = nlohmann::json::parse(line);
        } catch (...) {
            continue;
        }
        auto action = j.value("action", "");
        auto us = j.value("us", int64_t(0));
        if (action == "start") {
            auto type = j.value("type", 0);
            if (type != actBuild && type != actSubstitute)
                continue;
            auto & fields = j["fields"];
            if (!fields.is_array() || fields.empty() || !fields[0].is_string())
                continue;
            Span s;
            s.kind = type == actBuild ? "build" : "substitute";
            s.path = fields[0].get<std::string>();
            if (fields.size() > 1 && fields[1].is_string())
                s.machine = fields[1].get<std::string>();
            s.startUs = us;
            open[j.value("id", uint64_t(0))] = std::move(s);
        } else if (action == "stop") {
            auto it = open.find(j.value("id", uint64_t(0)));
            if (it == open.end())
                continue;
            it->second.endUs = us;
            done.push_back(std::move(it->second));
            open.erase(it);
        }
    }
    for (auto & [_, s] : open)
        done.push_back(std::move(s));

    /* A remote build is reported twice: once by the goal that dispatched it,
       naming the machine, and once by the build hook forwarding the remote
       side's own build activity, naming nothing. The second is nested inside
       the first and is the same work. Keep the outer span, and take the
       machine name from whichever of the two carries it. */
    std::sort(done.begin(), done.end(), [](const Span & a, const Span & b) { return a.startUs < b.startUs; });
    std::vector<Span> merged;
    for (auto & s : done) {
        auto * outer = enclosing(merged, s);
        if (outer) {
            if (outer->machine.empty())
                outer->machine = s.machine;
        } else
            merged.push_back(std::move(s));
    }
    return merged;
}

nlohmann::json spanJSON(const Span & s, int64_t fallbackEnd)
{
    nlohmann::json j;
    j["kind"] = s.kind;
    j["path"] = s.path;
    j["on"] = s.machine.empty() ? "local" : s.machine;
    j["startedAtUs"] = s.startUs;
    j["endedAtUs"] = s.endUs ? nlohmann::json(*s.endUs) : nlohmann::json(nullptr);
    j["seconds"] = s.seconds(fallbackEnd);
    return j;
}

struct CmdInvocationShow : Command, MixJSON
{
    std::string id = "last";

    CmdInvocationShow()
    {
        expectArgs({
            .label = "invocation-id",
            .optional = true,
            .handler = {&id},
        });
    }

    std::string description() override
    {
        return "show what a finished Nix invocation did";
    }

    std::string doc() override
    {
        return
#include "invocation-show.md"
            ;
    }

    Category category() override
    {
        return catUtility;
    }

    std::optional<ExperimentalFeature> experimentalFeature() override
    {
        return Xp::InvocationRecords;
    }

    void run() override
    {
        /* The multi-command dispatcher does not enforce a subcommand's
           feature, so require it here as `nix store builds` does. */
        experimentalFeatureSettings.require(Xp::InvocationRecords);

        auto dir = resolve(id);
        auto meta = readJSONFile(dir / "meta.json");
        auto stats = readJSONFile(dir / "eval-stats.json");
        auto spans = spansOf(dir / "events.jsonl");

        auto fallbackEnd = meta.value("endedAtUs", int64_t(0));
        std::sort(spans.begin(), spans.end(), [&](const Span & a, const Span & b) {
            return a.seconds(fallbackEnd) > b.seconds(fallbackEnd);
        });

        if (json) {
            nlohmann::json out = meta;
            out["recordDir"] = dir.string();
            out["eval"] = stats;
            auto & arr = out["work"] = nlohmann::json::array();
            for (auto & s : spans)
                arr.push_back(spanJSON(s, fallbackEnd));
            printJSON(out);
            return;
        }

        logger->cout("invocation %s", meta.value("invocationId", dir.filename().string()));
        logger->cout("  command   %s", meta.value("commandLine", "<unknown>"));
        logger->cout("  cwd       %s", meta.value("cwd", "<unknown>"));
        logger->cout("  nix       %s", meta.value("nixVersion", "<unknown>"));
        logger->cout("  wall      %.3f s", meta.value("wallSeconds", 0.0));
        logger->cout("  exit      %d", meta.value("exitStatus", 0));
        if (stats.contains("cpuTime")) {
            /* `cpuTime` is the whole process's user CPU when the evaluator
               reported, not evaluation alone. For a `nix build` the builders
               run in the daemon, so what is left is mostly evaluation. */
            logger->cout("  user cpu  %.3f s", stats.value("cpuTime", 0.0));
            if (stats.contains("time") && stats["time"].contains("gc"))
                logger->cout("  gc        %.3f s", stats["time"].value("gc", 0.0));
            if (stats.contains("gc"))
                logger->cout(
                    "  heap      %d bytes over %d cycles",
                    stats["gc"].value("heapSize", uint64_t(0)),
                    stats["gc"].value("cycles", uint64_t(0)));
            logger->cout(
                "  values    %d, thunks %d, calls %d",
                stats.contains("values") ? stats["values"].value("number", uint64_t(0)) : 0,
                stats.value("nrThunks", uint64_t(0)),
                stats.value("nrFunctionCalls", uint64_t(0)));
        } else
            logger->cout("  eval      not recorded");

        size_t builds = 0, substitutions = 0;
        double buildSeconds = 0;
        for (auto & s : spans) {
            if (s.kind == "build") {
                ++builds;
                buildSeconds += s.seconds(fallbackEnd);
            } else
                ++substitutions;
        }
        logger->cout("  builds    %d (%.3f s of builder time), substitutions %d", builds, buildSeconds, substitutions);

        if (spans.empty())
            return;
        logger->cout("");
        logger->cout("%12s  %-10s  %-14s  %s", "seconds", "kind", "on", "path");
        for (auto & s : spans)
            logger->cout(
                "%12.3f  %-10s  %-14s  %s",
                s.seconds(fallbackEnd),
                s.kind,
                s.machine.empty() ? "local" : s.machine,
                s.path);
    }
};

struct CmdInvocationList : Command, MixJSON
{
    std::string description() override
    {
        return "list the Nix invocations that were recorded";
    }

    Category category() override
    {
        return catUtility;
    }

    std::optional<ExperimentalFeature> experimentalFeature() override
    {
        return Xp::InvocationRecords;
    }

    void run() override
    {
        experimentalFeatureSettings.require(Xp::InvocationRecords);

        std::vector<std::string> names;
        std::error_code ec;
        for (auto & entry : std::filesystem::directory_iterator{recordsDir(), ec})
            names.push_back(entry.path().filename().string());
        std::sort(names.begin(), names.end());

        nlohmann::json out = nlohmann::json::array();
        for (auto & n : names) {
            auto meta = readJSONFile(recordsDir() / n / "meta.json");
            if (json) {
                meta["invocationId"] = n;
                out.push_back(meta);
            } else
                logger->cout(
                    "%s  %8.3f s  %s", n, meta.value("wallSeconds", 0.0), meta.value("commandLine", "<incomplete>"));
        }
        if (json)
            printJSON(out);
    }
};

struct CmdInvocation : NixMultiCommand
{
    CmdInvocation()
        : NixMultiCommand("invocation", RegisterCommand::getCommandsFor({"invocation"}))
    {
    }

    std::string description() override
    {
        return "inspect what a finished Nix invocation did";
    }

    Category category() override
    {
        return catUtility;
    }
};

auto rCmdInvocationShow = registerCommand2<CmdInvocationShow>({"invocation", "show"});
auto rCmdInvocationList = registerCommand2<CmdInvocationList>({"invocation", "list"});
auto rCmdInvocation = registerCommand<CmdInvocation>("invocation");

} // namespace
