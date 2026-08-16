#include "nix/expr/refusal-census.hh"
#include "nix/util/error.hh"

#include <iostream>
#include <mutex>

namespace nix {

namespace {

std::mutex & censusMutex()
{
    static std::mutex m;
    return m;
}

std::map<std::string, uint64_t> & counts()
{
    static std::map<std::string, uint64_t> c;
    return c;
}

uint64_t & totalCount()
{
    static uint64_t n = 0;
    return n;
}

std::string & commandName()
{
    static std::string name;
    return name;
}

std::string & lastRefusalToken()
{
    static std::string token;
    return token;
}

std::vector<std::string> & vocabularyStore()
{
    static std::vector<std::string> tokens;
    return tokens;
}

} // namespace

void RefusalCensus::record(std::string_view token, std::string_view detail)
{
    {
        auto lock = std::lock_guard{censusMutex()};
        counts()[std::string(token)]++;
        totalCount()++;
        lastRefusalToken() = std::string(token);
    }

    /* The `<4>` is not decoration and not a severity written in words.
       systemd hands journald `info` for any line that does not open with a
       syslog level prefix, so a message that says "warning" in its body is
       invisible to every query that filters on severity -- and a census whose
       rows cannot be selected by severity is a census nobody will find. The
       prefix is what makes this land as PRIORITY=4.

       Written straight to stderr rather than through nix's logger because the
       logger's own formatting would sit between the `<4>` and the start of
       the line, at which point journald sees an ordinary message again.

       Interactively the prefix is a small wart on the terminal. That is the
       trade: the line exists for the fleet, where it is the only channel that
       survives the process exiting on this very refusal. */
    std::cerr << "<4>rust-eval refusal token=" << token << " detail=" << detail << "\n";
    std::cerr.flush();
}

std::map<std::string, uint64_t> RefusalCensus::snapshot()
{
    auto lock = std::lock_guard{censusMutex()};
    return counts();
}

uint64_t RefusalCensus::total()
{
    auto lock = std::lock_guard{censusMutex()};
    return totalCount();
}

std::string RefusalCensus::lastToken()
{
    auto lock = std::lock_guard{censusMutex()};
    return lastRefusalToken();
}

void RefusalCensus::setVocabulary(std::vector<std::string> tokens)
{
    auto lock = std::lock_guard{censusMutex()};
    vocabularyStore() = std::move(tokens);
}

const std::vector<std::string> & RefusalCensus::vocabulary()
{
    auto lock = std::lock_guard{censusMutex()};
    return vocabularyStore();
}

void RefusingCommand::set(std::string_view name)
{
    auto lock = std::lock_guard{censusMutex()};
    commandName() = std::string(name);
}

std::string RefusingCommand::get()
{
    auto lock = std::lock_guard{censusMutex()};
    if (!commandName().empty())
        return commandName();
    /* Not "unknown" as a first choice and not the empty string: an unnamed
       row is the thing this whole mechanism exists to avoid, and `argv[0]` is
       already sitting there correct for every entry point that is not the
       `nix` multi-command. */
    return ErrorInfo::programName.value_or("unknown");
}

} // namespace nix
