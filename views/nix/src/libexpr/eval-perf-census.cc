#include "nix/expr/eval-perf-census.hh"

#include <mutex>

namespace nix {

namespace {

std::mutex & perfMutex()
{
    static std::mutex m;
    return m;
}

std::map<std::string, uint64_t> & perfCounts()
{
    static std::map<std::string, uint64_t> counts;
    return counts;
}

bool & perfSeen()
{
    static bool seen = false;
    return seen;
}

} // namespace

void EvalPerfCensus::record(std::string_view line)
{
    std::lock_guard<std::mutex> lock(perfMutex());
    perfSeen() = true;
    size_t pos = 0;
    while (pos < line.size()) {
        auto space = line.find(' ', pos);
        auto field = line.substr(pos, space == std::string_view::npos ? std::string_view::npos : space - pos);
        pos = space == std::string_view::npos ? line.size() : space + 1;
        auto eq = field.find('=');
        if (eq == std::string_view::npos)
            continue;
        auto key = std::string(field.substr(0, eq));
        auto value = field.substr(eq + 1);
        /* `ops_counted` is a bool and everything else is a count. Parsed with
           a hand-rolled loop rather than stoull because a malformed field
           must be skipped rather than throw: a stats block is diagnostic
           output and must not be able to fail an evaluation that already
           succeeded. */
        uint64_t n = 0;
        if (value == "true") {
            n = 1;
        } else if (value == "false") {
            n = 0;
        } else {
            bool ok = !value.empty();
            for (char c : value) {
                if (c < '0' || c > '9') {
                    ok = false;
                    break;
                }
                n = n * 10 + static_cast<uint64_t>(c - '0');
            }
            if (!ok)
                continue;
        }
        perfCounts()[key] += n;
    }
}

std::map<std::string, uint64_t> EvalPerfCensus::snapshot()
{
    std::lock_guard<std::mutex> lock(perfMutex());
    return perfCounts();
}

bool EvalPerfCensus::recorded()
{
    std::lock_guard<std::mutex> lock(perfMutex());
    return perfSeen();
}

} // namespace nix
