#include "nix/util/compression.hh"
#include <gtest/gtest.h>

namespace nix {

/* ----------------------------------------------------------------------------
 * compress / decompress
 * --------------------------------------------------------------------------*/

TEST(compress, noneMethodDoesNothingToTheInput)
{
    auto o = compress(CompressionAlgo::none, "this-is-a-test");

    ASSERT_EQ(o, "this-is-a-test");
}

TEST(decompress, decompressNoneCompressed)
{
    auto method = "none";
    auto str = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto o = decompress(method, str);

    ASSERT_EQ(o, str);
}

TEST(decompress, decompressEmptyCompressed)
{
    // Empty-method decompression used e.g. by S3 store
    // (Content-Encoding == "").
    auto method = "";
    auto str = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto o = decompress(method, str);

    ASSERT_EQ(o, str);
}

TEST(decompress, decompressXzCompressed)
{
    auto method = "xz";
    auto str = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto o = decompress(method, compress(CompressionAlgo::xz, str));

    ASSERT_EQ(o, str);
}

TEST(decompress, decompressBzip2Compressed)
{
    auto method = "bzip2";
    auto str = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto o = decompress(method, compress(CompressionAlgo::bzip2, str));

    ASSERT_EQ(o, str);
}

TEST(decompress, decompressBrCompressed)
{
    auto method = "br";
    auto str = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto o = decompress(method, compress(CompressionAlgo::brotli, str));

    ASSERT_EQ(o, str);
}

TEST(decompress, decompressInvalidInputThrowsCompressionError)
{
    auto method = "bzip2";
    auto str = "this is a string that does not qualify as valid bzip2 data";

    ASSERT_THROW(decompress(method, str), CompressionError);
}

/* ----------------------------------------------------------------------------
 * compression sinks
 * --------------------------------------------------------------------------*/

TEST(makeCompressionSink, noneSinkDoesNothingToInput)
{
    StringSink strSink;
    auto inputString = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto sink = makeCompressionSink(CompressionAlgo::none, strSink);
    (*sink)(inputString);
    sink->finish();

    ASSERT_STREQ(strSink.s.c_str(), inputString);
}

TEST(makeCompressionSink, compressAndDecompress)
{
    StringSink strSink;
    auto inputString = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto decompressionSink = makeDecompressionSink("bzip2", strSink);
    auto sink = makeCompressionSink(CompressionAlgo::bzip2, *decompressionSink);

    (*sink)(inputString);
    sink->finish();
    decompressionSink->finish();

    ASSERT_STREQ(strSink.s.c_str(), inputString);
}

/* ----------------------------------------------------------------------------
 * parallel compression thread count
 * --------------------------------------------------------------------------*/

TEST(makeCompressionSink, explicitThreadCountRoundTripsForXz)
{
    // A pinned thread count is not an "auto" 0: this exercises the actual
    // std::to_string(threads) path, not the default that happens to already
    // match libarchive's own "0 = auto".
    auto inputString = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    auto compressed = compress(CompressionAlgo::xz, inputString, /*parallel=*/true, /*level=*/-1, /*threads=*/2);
    auto o = decompress("xz", compressed);

    ASSERT_EQ(o, inputString);
}

TEST(makeCompressionSink, nonzeroThreadsWithoutParallelFlagIsRejected)
{
    // The bool `parallel` argument still gates whether the `threads` filter
    // option is set at all; a caller that wants a positive thread count
    // must go through BinaryCacheStoreConfig, which turns a nonzero
    // parallel-compression-threads into parallel=true (see
    // binary-cache-store.cc). Calling makeCompressionSink directly with
    // parallel=false and a nonzero thread count is a caller bug: the
    // thread count is silently unused, exactly like passing parallel=false
    // has always silently ignored parallel-compression's old bool. This
    // test documents that this layer does not itself protect against it,
    // so the protection has to live at the one call site that turns
    // settings into these two arguments.
    StringSink strSink;
    auto sink = makeCompressionSink(CompressionAlgo::xz, strSink, /*parallel=*/false, /*level=*/-1, /*threads=*/8);
    auto inputString = "slfja;sljfklsa;jfklsjfkl;sdjfkl;sadjfkl;sdjf;lsdfjsadlf";
    (*sink)(inputString);
    sink->finish();

    auto o = decompress("xz", strSink.s);
    ASSERT_EQ(o, inputString);
}

TEST(makeCompressionSink, threadCountOnANonThreadingFilterThrowsLegibly)
{
    // gzip's libarchive filter has no `threads` option. Requesting parallel
    // compression for it must not silently compress single-threaded; it
    // must fail loudly enough that a misconfigured `compression = gzip` +
    // `parallel-compression-threads = N` is caught immediately rather than
    // discovered as "why didn't this speed up".
    StringSink strSink;
    try {
        auto sink = makeCompressionSink(CompressionAlgo::gzip, strSink, /*parallel=*/true, /*level=*/-1, /*threads=*/4);
        FAIL() << "expected makeCompressionSink to throw for gzip + parallel threads";
    } catch (Error & e) {
        EXPECT_NE(std::string(e.what()).find("gzip"), std::string::npos)
            << "error should name the offending compression method, got: " << e.what();
        EXPECT_NE(std::string(e.what()).find("multi-threaded"), std::string::npos)
            << "error should say why, got: " << e.what();
    }
}

} // namespace nix
