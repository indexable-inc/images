#pragma once
///@file

#include <functional>
#include <limits>
#include <string>

#include "nix/util/file-descriptor.hh"

namespace nix {

/**
 * Determine whether \param fd is a terminal.
 */
bool isTTY(Descriptor fd);

/**
 * Determine whether ANSI escape sequences are appropriate for the
 * present output.
 */
bool isTTY();

/**
 * Truncate a string to 'width' printable characters. If 'filterAll'
 * is true, all ANSI escape sequences are filtered out. Otherwise,
 * some escape sequences (such as colour setting) are copied but not
 * included in the character count. Also, tabs are expanded to
 * spaces.
 */
std::string filterANSIEscapes(
    std::string_view s, bool filterAll = false, unsigned int width = std::numeric_limits<unsigned int>::max());

/**
 * Recalculate the window size, updating a global variable.
 *
 * Used in the `SIGWINCH` signal handler on Unix, for example.
 */
void updateWindowSize();

/**
 * @return the number of rows and columns of the terminal.
 *
 * The value is cached so this is quick. The cached result is computed
 * by `updateWindowSize()`.
 */
std::pair<unsigned short, unsigned short> getWindowSize();

/**
 * Register a callback invoked after `updateWindowSize()` refreshes the
 * cached size, i.e. on `SIGWINCH`. The callback runs on the signal
 * handler thread (a real thread, not an async signal handler), so it
 * may take locks. Pass an empty function to unregister; the previous
 * callback has finished running by the time this returns.
 */
void setWindowSizeCallback(std::function<void()> cb);

/**
 * Get the slave name of a pseudoterminal in a thread-safe manner.
 *
 * @param fd The file descriptor of the pseudoterminal master
 * @return The slave device name as a string
 */
std::string getPtsName(int fd);

} // namespace nix
