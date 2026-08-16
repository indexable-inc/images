#include <gtest/gtest.h>

#include "nix/store/build/build-log.hh"
#include "nix/util/logging.hh"

namespace nix {

/**
 * Captures what the logger was asked to print, so a test can assert on the
 * absence of a message as well as its presence.
 */
struct CapturingLogger : Logger
{
    std::vector<std::string> errors;

    void log(Verbosity lvl, std::string_view s) override
    {
        if (lvl <= lvlError)
            errors.emplace_back(s);
    }

    void logEI(const ErrorInfo & ei) override
    {
        errors.emplace_back(ei.msg.str());
    }

    void result(ActivityId act, ResultType type, const Fields & fields) override {}
};

struct BuildLogTest : ::testing::Test
{
    CapturingLogger capturing;
    Logger * saved = nullptr;

    void SetUp() override
    {
        saved = logger.release();
        logger.reset(&capturing);
    }

    void TearDown() override
    {
        logger.release();
        logger.reset(saved);
    }

    static std::unique_ptr<BuildLog> makeLog()
    {
        return std::make_unique<BuildLog>(10, std::make_unique<Activity>(*logger, lvlInfo, actBuild, "test"));
    }

    bool sawBadJSON() const
    {
        for (auto & e : capturing.errors)
            if (e.find("bad JSON log message") != std::string::npos)
                return true;
        return false;
    }
};

/**
 * A builder whose output stops midway through a `@nix {...}` line has not
 * emitted a malformed message; it has emitted no message. Reporting a JSON
 * parse error there blames the builder for a truncation it did not cause, and
 * that is what a real `nix run` over 992 derivations printed seven times:
 *
 *   bad JSON log message from the derivation builder: parse error at line 1,
 *   column 26: ... missing closing quote; last read: '"ac'
 */
TEST_F(BuildLogTest, unterminatedJSONLineIsNotParsed)
{
    auto log = makeLog();
    (*log)(R"(@nix {"action":"setPhase","pha)");
    log->flush();

    EXPECT_FALSE(sawBadJSON());
}

/**
 * The partial line must still reach the tail, because it is the last thing the
 * builder said and is often the reason it stopped.
 */
TEST_F(BuildLogTest, unterminatedJSONLineIsKeptAsText)
{
    auto log = makeLog();
    (*log)(R"(@nix {"action":"setPhase","pha)");
    log->flush();

    ASSERT_EQ(log->getTail().size(), 1u);
    EXPECT_EQ(log->getTail().back(), R"(@nix {"action":"setPhase","pha)");
}

/**
 * A complete JSON line is still consumed as a message rather than printed, so
 * the fix does not turn structured phase reporting back into log noise.
 */
TEST_F(BuildLogTest, terminatedJSONLineIsStillHandled)
{
    auto log = makeLog();
    (*log)("@nix {\"action\":\"setPhase\",\"phase\":\"installPhase\"}\n");
    log->flush();

    EXPECT_FALSE(sawBadJSON());
    EXPECT_TRUE(log->getTail().empty());
}

/**
 * A terminated line that really is malformed must still be reported: the fix
 * narrows what is parsed, and must not stop parsing altogether.
 */
TEST_F(BuildLogTest, terminatedMalformedJSONStillReports)
{
    auto log = makeLog();
    (*log)("@nix {\"action\":\"setPha\n");
    log->flush();

    EXPECT_TRUE(sawBadJSON());
}

/**
 * A carriage return rewinds the write position without shrinking the buffer, so
 * a buffer holding stale bytes past that position must not be flushed as if the
 * builder had written them.
 */
TEST_F(BuildLogTest, carriageReturnLeavesNothingToFlush)
{
    auto log = makeLog();
    (*log)("a long progress line\r");
    log->flush();

    EXPECT_FALSE(sawBadJSON());
    EXPECT_TRUE(log->getTail().empty());
}

/**
 * A line split across reads is one line, not two, and is complete once its
 * newline arrives.
 */
TEST_F(BuildLogTest, jsonSplitAcrossWritesIsStillOneMessage)
{
    auto log = makeLog();
    (*log)("@nix {\"action\":\"set");
    (*log)("Phase\",\"phase\":\"installPhase\"}\n");
    log->flush();

    EXPECT_FALSE(sawBadJSON());
    EXPECT_TRUE(log->getTail().empty());
}

} // namespace nix
