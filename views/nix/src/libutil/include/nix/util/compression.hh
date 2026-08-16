#pragma once
///@file

#include "nix/util/ref.hh"
#include "nix/util/types.hh"
#include "nix/util/serialise.hh"
#include "nix/util/compression-algo.hh"

#include <string>

namespace nix {

struct CompressionSink : BufferedSink, FinishSink
{
    using BufferedSink::operator();
    using BufferedSink::writeUnbuffered;
    using FinishSink::finish;
};

std::string decompress(const std::string & method, std::string_view in);

std::unique_ptr<FinishSink> makeDecompressionSink(const std::string & method, Sink & nextSink);

std::string compress(
    CompressionAlgo method, std::string_view in, const bool parallel = false, int level = -1, unsigned int threads = 0);

/**
 * @param threads Passed through to libarchive's filter-specific `threads`
 * option when `parallel` is set. `0` (the default) means "as many threads
 * as libarchive thinks the hardware has", which is the long-standing
 * meaning of `parallel = true` on its own. A positive value pins the
 * thread count instead, so a caller does not have to choose between one
 * thread and every core. Only `xz` and `zstd` filters accept this option;
 * setting it (implicitly, by passing a nonzero value, or explicitly via
 * `parallel`) for another method throws a legible error at compression
 * time rather than silently doing nothing.
 */
ref<CompressionSink> makeCompressionSink(
    CompressionAlgo method, Sink & nextSink, const bool parallel = false, int level = -1, unsigned int threads = 0);

MakeError(CompressionError, Error);

} // namespace nix
