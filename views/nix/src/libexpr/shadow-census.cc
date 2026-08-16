#include "nix/expr/shadow-census.hh"

#include <algorithm>
#include <iostream>
#include <mutex>

namespace nix {

namespace {

std::mutex & shadowMutex()
{
    static std::mutex m;
    return m;
}

struct State
{
    uint64_t attempts = 0;
    uint64_t micros = 0;
    std::map<ShadowVerdict, uint64_t> verdicts;
    std::map<std::string, uint64_t> refusalTokens;
    std::map<ShadowSkip, uint64_t> skips;
    std::map<std::string, uint64_t> kinds;
    std::vector<ShadowCensus::Divergence> divergences;
};

State & state()
{
    static State s;
    return s;
}

} // namespace

std::string_view shadowVerdictName(ShadowVerdict verdict)
{
    switch (verdict) {
    case ShadowVerdict::Agreed:
        return "agreed";
    case ShadowVerdict::AgreedFailure:
        return "agreed-failure";
    case ShadowVerdict::AgreedFailureTextDiffers:
        return "agreed-failure-text-differs";
    case ShadowVerdict::Refused:
        return "refused";
    case ShadowVerdict::Mismatched:
        return "mismatched";
    case ShadowVerdict::Crashed:
        return "crashed";
    case ShadowVerdict::TimedOut:
        return "timed-out";
    }
    /* Not reachable through the enum, and not an invented name either: a
       verdict with no name would be a histogram row nobody can explain. */
    return "unnamed-verdict";
}

const std::vector<ShadowVerdict> & allShadowVerdicts()
{
    static const std::vector<ShadowVerdict> all{
        ShadowVerdict::Agreed,
        ShadowVerdict::AgreedFailure,
        ShadowVerdict::AgreedFailureTextDiffers,
        ShadowVerdict::Refused,
        ShadowVerdict::Mismatched,
        ShadowVerdict::Crashed,
        ShadowVerdict::TimedOut,
    };
    return all;
}

std::string_view shadowSkipName(ShadowSkip why)
{
    switch (why) {
    case ShadowSkip::Reentrant:
        return "reentrant";
    case ShadowSkip::Budget:
        return "budget";
    case ShadowSkip::CppFailedBeforeEval:
        return "cpp-failed-before-eval";
    case ShadowSkip::UnservableShape:
        return "unservable-shape";
    case ShadowSkip::NonValueInstallable:
        return "non-value-installable";
    case ShadowSkip::FlakeUnservable:
        return "flake-unservable";
    case ShadowSkip::CppAnswerShape:
        return "cpp-answer-shape";
    case ShadowSkip::BackendAbsent:
        return "backend-absent";
    }
    return "unnamed-skip";
}

const std::vector<ShadowSkip> & allShadowSkips()
{
    static const std::vector<ShadowSkip> all{
        ShadowSkip::Reentrant,
        ShadowSkip::Budget,
        ShadowSkip::CppFailedBeforeEval,
        ShadowSkip::UnservableShape,
        ShadowSkip::NonValueInstallable,
        ShadowSkip::FlakeUnservable,
        ShadowSkip::CppAnswerShape,
        ShadowSkip::BackendAbsent,
    };
    return all;
}

const std::vector<std::string_view> & allShadowKinds()
{
    static const std::vector<std::string_view> all{
        shadowKinds::valueMismatch,
        shadowKinds::errorClassMismatch,
        shadowKinds::errorClassLost,
        shadowKinds::errorTextMismatch,
        shadowKinds::rustFailed,
        shadowKinds::cppFailed,
        shadowKinds::rustCrashed,
    };
    return all;
}

void ShadowCensus::attempt()
{
    auto lock = std::lock_guard{shadowMutex()};
    state().attempts++;
}

void ShadowCensus::record(ShadowVerdict verdict, std::string_view token)
{
    auto lock = std::lock_guard{shadowMutex()};
    state().verdicts[verdict]++;
    if (verdict == ShadowVerdict::Refused)
        /* An empty token would be a row with no name, and the vocabulary
           already has a word for "nobody said": `unrecorded`. Spelled here
           rather than pulled from rust-eval-refusal.hh, which this file must
           not depend on -- that header includes the census, not the other way
           round. */
        state().refusalTokens[token.empty() ? std::string("unrecorded") : std::string(token)]++;
}

void ShadowCensus::skipped(ShadowSkip why)
{
    auto lock = std::lock_guard{shadowMutex()};
    state().skips[why]++;
}

void ShadowCensus::spent(uint64_t micros)
{
    auto lock = std::lock_guard{shadowMutex()};
    state().micros += micros;
}

bool ShadowCensus::budgetExhausted(uint64_t budgetMicros)
{
    if (budgetMicros == 0)
        return false;
    auto lock = std::lock_guard{shadowMutex()};
    return state().micros >= budgetMicros;
}

void ShadowCensus::diverged(
    std::string_view kind, const std::string & id, const std::string & origin, const std::string & detail)
{
    {
        auto lock = std::lock_guard{shadowMutex()};
        auto & s = state();
        s.kinds[std::string(kind)]++;
        auto it =
            std::find_if(s.divergences.begin(), s.divergences.end(), [&](const Divergence & d) { return d.id == id; });
        if (it != s.divergences.end())
            it->count++;
        else if (s.divergences.size() < maxRememberedDivergences)
            s.divergences.push_back(Divergence{std::string(kind), id, origin, detail, 1});
    }

    /* The `<4>` for the reason `RefusalCensus::record` gives: systemd hands
       journald `info` for any line with no syslog level prefix, so a
       divergence that merely says "warning" in its text is invisible to every
       severity-filtered query -- and a divergence nobody queries is a
       divergence nobody fixes. Straight to stderr rather than through the
       logger, because the logger's formatting would sit between the prefix
       and the start of the line.

       One line, and the same fields every time, so this is greppable and
       groupable without a parser: `id` is what groups the same divergence
       across machines and runs, `kind` is the histogram row, `origin` says
       where to look, and `detail` carries both truncated results. */
    std::cerr << "<4>rust-eval shadow divergence kind=" << kind << " id=" << id << " origin=" << origin << " " << detail
              << "\n";
    std::cerr.flush();
}

uint64_t ShadowCensus::attempts()
{
    auto lock = std::lock_guard{shadowMutex()};
    return state().attempts;
}

uint64_t ShadowCensus::micros()
{
    auto lock = std::lock_guard{shadowMutex()};
    return state().micros;
}

std::map<std::string, uint64_t> ShadowCensus::verdicts()
{
    auto lock = std::lock_guard{shadowMutex()};
    std::map<std::string, uint64_t> out;
    /* Every verdict, present at zero. A histogram whose absent rows mean
       "none" and whose missing rows mean "this build cannot count it" reads
       identically, and the flip criterion is a claim about zeros. */
    for (auto verdict : allShadowVerdicts())
        out[std::string(shadowVerdictName(verdict))] = 0;
    for (const auto & [verdict, n] : state().verdicts)
        out[std::string(shadowVerdictName(verdict))] = n;
    return out;
}

std::map<std::string, uint64_t> ShadowCensus::refusalTokens()
{
    auto lock = std::lock_guard{shadowMutex()};
    return state().refusalTokens;
}

std::map<std::string, uint64_t> ShadowCensus::skips()
{
    auto lock = std::lock_guard{shadowMutex()};
    std::map<std::string, uint64_t> out;
    for (auto why : allShadowSkips())
        out[std::string(shadowSkipName(why))] = 0;
    for (const auto & [why, n] : state().skips)
        out[std::string(shadowSkipName(why))] = n;
    return out;
}

std::map<std::string, uint64_t> ShadowCensus::divergenceKinds()
{
    auto lock = std::lock_guard{shadowMutex()};
    std::map<std::string, uint64_t> out;
    for (auto kind : allShadowKinds())
        out[std::string(kind)] = 0;
    for (const auto & [kind, n] : state().kinds)
        out[kind] = n;
    return out;
}

uint64_t ShadowCensus::unaccounted()
{
    auto lock = std::lock_guard{shadowMutex()};
    uint64_t concluded = 0;
    for (const auto & [_, n] : state().verdicts)
        concluded += n;
    return state().attempts > concluded ? state().attempts - concluded : 0;
}

std::vector<ShadowCensus::Divergence> ShadowCensus::divergences()
{
    auto lock = std::lock_guard{shadowMutex()};
    return state().divergences;
}

} // namespace nix
