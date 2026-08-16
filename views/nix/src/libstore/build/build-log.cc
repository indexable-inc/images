#include "nix/store/build/build-log.hh"

namespace nix {

BuildLog::BuildLog(size_t maxTailLines, std::unique_ptr<Activity> act)
    : maxTailLines(maxTailLines)
    , act(std::move(act))
{
}

void BuildLog::operator()(std::string_view data)
{
    for (auto c : data)
        if (c == '\r')
            currentLogLinePos = 0;
        else if (c == '\n')
            flushLine(true);
        else {
            if (currentLogLinePos >= currentLogLine.size())
                currentLogLine.resize(currentLogLinePos + 1);
            currentLogLine[currentLogLinePos++] = c;
        }
}

void BuildLog::flush()
{
    /* Whatever is left has no newline after it, because a newline would have
       flushed it already. It is therefore not a complete log message, and must
       not be offered to the JSON parser: a builder whose output stops midway
       through a `@nix {...}` line would otherwise be reported as having emitted
       malformed JSON, when all that happened is that it stopped writing.

       That misattribution is what this guards against, verbatim from a real
       session, seven of them during one `nix run` over 992 derivations:

         bad JSON log message from the derivation builder:
           parse error at line 1, column 26: syntax error while parsing object
           key - invalid string: missing closing quote; last read: '"ac'

       Tested on currentLogLinePos rather than the buffer, because a carriage
       return rewinds the position without shrinking the buffer, so a non-empty
       buffer can still hold nothing the builder meant to say. */
    if (currentLogLinePos > 0)
        flushLine(false);
}

void BuildLog::flushLine(bool lineComplete)
{
    // Truncate to actual content (currentLogLinePos may be less than size due to \r)
    currentLogLine.resize(currentLogLinePos);

    if (!lineComplete
        || !handleJSONLogMessage(currentLogLine, *act, builderActivities, "the derivation builder", false)) {
        // Line was not handled as JSON, emit and add to tail
        act->result(resBuildLogLine, currentLogLine);
        logTail.push_back(currentLogLine);
        if (logTail.size() > maxTailLines)
            logTail.pop_front();
    }

    currentLogLine.clear();
    currentLogLinePos = 0;
}

} // namespace nix
