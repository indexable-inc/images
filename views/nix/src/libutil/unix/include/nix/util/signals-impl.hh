#pragma once
/**
 * @file
 *
 * Implementation of some inline definitions for Unix signals, and also
 * some extra Unix-only interfaces.
 *
 * (The only reason everything about signals isn't Unix-only is some
 * no-op definitions are provided on Windows to avoid excess CPP in
 * downstream code.)
 */

#include "nix/util/types.hh"
#include "nix/util/error.hh"
#include "nix/util/logging.hh"
#include "nix/util/ansicolor.hh"

#include <sys/types.h>
#include <sys/stat.h>
#include <dirent.h>
#include <unistd.h>
#include <signal.h>

#include <boost/lexical_cast.hpp>

#include <atomic>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>

namespace nix {

/* User interruption. */

namespace unix {

extern std::atomic<bool> _isInterrupted;

[[gnu::tls_model("initial-exec")]] extern thread_local std::function<bool()> interruptCheck;

void _interrupted();

/**
 * Whether startSignalHandlerThread() should replace the saved pre-handler
 * signal mask. A forked daemon worker inherits that saved mask but loses the
 * signal thread, so replacing it with the already blocked mask would prevent
 * its children from restoring normal signal delivery.
 */
enum class SignalMaskSave {
    Save,
    Keep,
};

/**
 * Start a thread that handles various signals. Also block those signals
 * on the current thread (and thus any threads created by it).
 * Optionally saves the signal mask before changing the mask to block those
 * signals.
 * See saveSignalMask().
 */
void startSignalHandlerThread(SignalMaskSave save = SignalMaskSave::Save);

/**
 * Saves the signal mask, which is the signal mask that nix will restore
 * before creating child processes.
 */
void saveSignalMask();

/**
 * To use in a process that already called `startSignalHandlerThread()`
 * or `saveSignalMask()` first.
 */
void restoreSignals();

void triggerInterrupt();

} // namespace unix

static inline void setInterrupted(bool isInterrupted)
{
    unix::_isInterrupted = isInterrupted;
}

static inline bool getInterrupted()
{
    return unix::_isInterrupted;
}

static inline bool isInterrupted()
{
    using namespace unix;
    return _isInterrupted || (interruptCheck && interruptCheck());
}

/**
 * Throw `Interrupted` exception if the process has been interrupted.
 *
 * Call this in long-running loops and between slow operations to terminate
 * them as needed.
 */
inline void checkInterrupt()
{
    if (isInterrupted())
        unix::_interrupted();
}

/**
 * A RAII class that causes the current thread to receive SIGUSR1 when
 * the signal handler thread receives SIGINT. That is, this allows
 * SIGINT to be multiplexed to multiple threads.
 *
 * Callback invocation is serialized with destruction because
 * triggerInterrupt() invokes a copied callback after releasing the registry
 * lock.
 */
struct ReceiveInterrupts
{
    struct State
    {
        std::mutex mutex;
        std::optional<pthread_t> target = pthread_self();
    };

    std::shared_ptr<State> state;
    std::unique_ptr<InterruptCallback> callback;

    ReceiveInterrupts()
        : state(std::make_shared<State>())
        , callback(createInterruptCallback([state = state]() {
            std::lock_guard lock(state->mutex);
            if (state->target)
                pthread_kill(*state->target, SIGUSR1);
        }))
    {
    }

    ~ReceiveInterrupts()
    {
        std::lock_guard lock(state->mutex);
        state->target.reset();
    }
};

} // namespace nix
