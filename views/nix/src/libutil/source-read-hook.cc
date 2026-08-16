#include "nix/util/source-read-hook.hh"

namespace nix {

std::atomic<SourceReadHook> sourceReadHook{nullptr};
std::atomic<SourceObservedHook> sourceObservedHook{nullptr};

} // namespace nix
