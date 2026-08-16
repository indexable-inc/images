#include "nix/store/darwin-compression.hh"

#ifdef __APPLE__

#  include "nix/util/file-system.hh"
#  include "nix/util/logging.hh"
#  include "nix/util/serialise.hh"
#  include "nix/util/signals.hh"

#  include <compression.h>
#  include <sys/attr.h>
#  include <sys/stat.h>
#  include <sys/xattr.h>
#  include <unistd.h>
#  include <fcntl.h>

namespace nix {

namespace {

/* The decmpfs on-disk format, as implemented by xnu's bsd/kern/decmpfs.c and
   the AppleFSCompression decompressors. See bsd/sys/decmpfs.h.

   A compressed file carries the `com.apple.decmpfs` xattr (16-byte header:
   little-endian magic, type, uncompressed size) and the UF_COMPRESSED BSD
   flag. Small payloads live inline in that xattr; larger ones live in the
   resource fork as independently-compressed 64 KiB chunks preceded by a table
   of absolute offsets. The kernel decompresses on read, so the file's contents
   are unchanged as far as every reader (including `nix hash path`) is
   concerned; only its on-disk footprint shrinks. */

constexpr uint32_t decmpfsMagic = 0x636d7066; /* 'cmpf' */
constexpr uint32_t decmpfsTypeLzfseXattr = 11;
constexpr uint32_t decmpfsTypeLzfseResourceFork = 12;
constexpr size_t decmpfsHeaderSize = 16;
constexpr size_t decmpfsMaxXattrSize = 3802;
constexpr size_t decmpfsChunkSize = 0x10000;
/* A chunk that does not shrink is stored verbatim behind this marker byte. */
constexpr uint8_t lzfseStoredMarker = 0xff;

constexpr const char * decmpfsXattrName = "com.apple.decmpfs";
constexpr const char * resourceForkXattrName = "com.apple.ResourceFork";

void putLE32(std::string & out, uint32_t v)
{
    out.push_back((char) (v & 0xff));
    out.push_back((char) ((v >> 8) & 0xff));
    out.push_back((char) ((v >> 16) & 0xff));
    out.push_back((char) ((v >> 24) & 0xff));
}

void putLE64(std::string & out, uint64_t v)
{
    putLE32(out, (uint32_t) (v & 0xffffffff));
    putLE32(out, (uint32_t) (v >> 32));
}

/** Compress one chunk, falling back to a verbatim copy behind the stored
    marker when compression does not pay for itself. */
std::string compressChunk(std::string_view src)
{
    std::string out;
    out.resize(src.size() + 16);
    auto n = compression_encode_buffer(
        (uint8_t *) out.data(), out.size(), (const uint8_t *) src.data(), src.size(), nullptr, COMPRESSION_LZFSE);
    if (n > 0 && n < src.size()) {
        out.resize(n);
        return out;
    }
    std::string stored;
    stored.reserve(src.size() + 1);
    stored.push_back((char) lzfseStoredMarker);
    stored.append(src);
    return stored;
}

} // namespace

bool compressPathIfWorthwhile(const std::filesystem::path & path, CompressionStats * stats, bool dryRun)
{
    auto st = maybeLstat(path);
    if (!st || !S_ISREG(st->st_mode))
        return false;

    /* Files that already carry the flag are done: this is what makes a sweep
       over the whole store idempotent and cheap to re-run. */
    if (st->st_flags & UF_COMPRESSED) {
        if (stats)
            stats->filesAlreadyCompressed++;
        return false;
    }

    /* Empty files have nothing to gain. */
    if (st->st_size == 0)
        return false;

    /* Compressing a hard-linked file would change the on-disk representation
       of every other name for that inode, and the store's optimiser relies on
       link counts; leave those to the optimiser. */
    if (st->st_nlink != 1) {
        if (stats) {
            stats->filesHardLinked++;
            stats->bytesHardLinked += (uint64_t) st->st_size;
        }
        return false;
    }

    std::string contents;
    try {
        contents = readFile(path);
    } catch (SystemError &) {
        return false;
    }
    if (contents.size() != (size_t) st->st_size)
        return false;

    /* Build the payload. */
    std::string xattr, resourceFork;
    uint32_t type;

    auto chunkCount = (contents.size() + decmpfsChunkSize - 1) / decmpfsChunkSize;
    std::vector<std::string> chunks;
    chunks.reserve(chunkCount);
    size_t totalCompressed = 0;
    for (size_t off = 0; off < contents.size(); off += decmpfsChunkSize) {
        auto chunk =
            compressChunk(std::string_view(contents).substr(off, std::min(decmpfsChunkSize, contents.size() - off)));
        totalCompressed += chunk.size();
        chunks.push_back(std::move(chunk));
    }

    if (chunkCount == 1 && decmpfsHeaderSize + totalCompressed <= decmpfsMaxXattrSize) {
        type = decmpfsTypeLzfseXattr;
    } else {
        type = decmpfsTypeLzfseResourceFork;
        /* `chunkCount + 1` absolute offsets, then the chunks themselves. */
        auto tableSize = 4 * (chunkCount + 1);
        resourceFork.reserve(tableSize + totalCompressed);
        uint32_t offset = (uint32_t) tableSize;
        for (auto & chunk : chunks) {
            putLE32(resourceFork, offset);
            offset += (uint32_t) chunk.size();
        }
        putLE32(resourceFork, offset);
        for (auto & chunk : chunks)
            resourceFork.append(chunk);
    }

    /* Only proceed if we actually save allocated blocks. The filesystem
       allocates in blocks, so a marginal byte-level win is no win at all;
       require the payload to be at least one block smaller. */
    auto blockSize = (size_t) std::max<blksize_t>(st->st_blksize, 4096);
    auto payloadSize = type == decmpfsTypeLzfseXattr ? decmpfsHeaderSize + totalCompressed : resourceFork.size();
    auto blocksBefore = (contents.size() + blockSize - 1) / blockSize;
    auto blocksAfter = (payloadSize + blockSize - 1) / blockSize;
    if (blocksAfter >= blocksBefore)
        return false;

    auto estimatedSaving = (uint64_t) (blocksBefore - blocksAfter) * blockSize;

    /* A dry run has everything it came for: the exact payload the real run
       would write, and hence the blocks it would free. Touch nothing. */
    if (dryRun) {
        if (stats) {
            stats->filesCompressed++;
            stats->bytesSaved += estimatedSaving;
        }
        return true;
    }

    xattr.reserve(decmpfsHeaderSize + (type == decmpfsTypeLzfseXattr ? totalCompressed : 0));
    putLE32(xattr, decmpfsMagic);
    putLE32(xattr, type);
    putLE64(xattr, (uint64_t) contents.size());
    if (type == decmpfsTypeLzfseXattr)
        xattr.append(chunks.front());

    /* Store files are canonicalised to mode 0444/0555 and mtime 1 before we
       get here. setxattr(2) requires write permission, and writing the xattrs
       bumps the mtime, so both have to be restored: the canonical mtime is an
       invariant the store asserts on when it canonicalises a path again.
       Restoration happens by every exit path, including the failures below, so
       what the caller registered is what stays on disk. */
    struct RestoreMetadata
    {
        const std::filesystem::path & path;
        mode_t mode;
        struct timespec times[2];
        bool restoreMode;

        ~RestoreMetadata()
        {
            /* The raw syscalls, not nix's throwing wrappers: a destructor must
               not throw. Nothing useful remains to be done if these fail, and
               a wrong mode or mtime is visible to the caller's own checks. */
            if (restoreMode)
                ::chmod(path.c_str(), mode);
            ::utimensat(AT_FDCWD, path.c_str(), times, AT_SYMLINK_NOFOLLOW);
        }
    };

    auto failed = [&]() {
        if (stats)
            stats->filesFailed++;
        return false;
    };

    mode_t origMode = st->st_mode & ~S_IFMT;
    RestoreMetadata restore{
        path,
        origMode,
        {st->st_atimespec, st->st_mtimespec},
        !(origMode & S_IWUSR),
    };
    if (restore.restoreMode && ::chmod(path.c_str(), origMode | S_IWUSR) != 0)
        return failed();

    /* Write order is mandated by the kernel: the decmpfs xattr (and resource
       fork) must be in place *before* UF_COMPRESSED is set, at which point the
       kernel itself truncates the data fork. See decmpfs_update_attributes(). */
    auto cleanup = [&]() {
        removexattr(path.c_str(), decmpfsXattrName, XATTR_NOFOLLOW | XATTR_SHOWCOMPRESSION);
        removexattr(path.c_str(), resourceForkXattrName, XATTR_NOFOLLOW | XATTR_SHOWCOMPRESSION);
    };

    if (!resourceFork.empty()
        && setxattr(
               path.c_str(),
               resourceForkXattrName,
               resourceFork.data(),
               resourceFork.size(),
               0,
               XATTR_NOFOLLOW | XATTR_SHOWCOMPRESSION)
               != 0) {
        /* A partial resource fork may have been written before the failure,
           and UF_COMPRESSED was never set, so nothing reads it -- but leaving
           it behind would waste the space this call was meant to save. */
        cleanup();
        return failed();
    }

    if (setxattr(path.c_str(), decmpfsXattrName, xattr.data(), xattr.size(), 0, XATTR_NOFOLLOW | XATTR_SHOWCOMPRESSION)
        != 0) {
        cleanup();
        return failed();
    }

    if (chflags(path.c_str(), st->st_flags | UF_COMPRESSED) != 0) {
        cleanup();
        return failed();
    }

    /* Verify by reading the file back through the kernel's decompressor. A
       store path whose bytes changed is a catastrophe, not a missed
       optimisation, so this check is unconditional: if the round-trip is not
       byte-identical we restore the original contents and give up. Note that
       clearing UF_COMPRESSED does NOT restore the data (the kernel truncated
       the data fork), so recovery must rewrite the file. */
    bool ok = false;
    try {
        ok = readFile(path) == contents;
    } catch (SystemError &) {
        ok = false;
    }

    if (!ok) {
        warn("APFS compression of %s did not round-trip; restoring the file uncompressed", PathFmt(path));
        chflags(path.c_str(), st->st_flags & ~UF_COMPRESSED);
        cleanup();
        {
            AutoCloseFD fd{open(path.c_str(), O_WRONLY | O_TRUNC | O_CLOEXEC)};
            if (!fd)
                throw SysError("reopening %s to restore its contents", PathFmt(path));
            writeFull(fd.get(), contents);
        }
        if (readFile(path) != contents)
            throw Error("failed to restore the contents of %s after a failed compression attempt", PathFmt(path));
        return failed();
    }

    if (stats) {
        stats->filesCompressed++;
        /* Prefer the measured allocation delta (st_blocks counts the resource
           fork too); fall back to the block estimate if the re-stat fails. */
        auto after = maybeLstat(path);
        if (after && after->st_blocks < st->st_blocks)
            stats->bytesSaved += (uint64_t) (st->st_blocks - after->st_blocks) * 512;
        else
            stats->bytesSaved += estimatedSaving;
    }

    return true;
}

void compressPathRecursively(const std::filesystem::path & path, CompressionStats & stats, bool dryRun)
{
    checkInterrupt();

    auto st = maybeLstat(path);
    if (!st)
        return;

    if (S_ISDIR(st->st_mode)) {
        for (auto & i : DirectoryIterator{path})
            compressPathRecursively(i.path(), stats, dryRun);
    } else if (S_ISREG(st->st_mode)) {
        stats.filesScanned++;
        compressPathIfWorthwhile(path, &stats, dryRun);
    }
}

} // namespace nix

#endif
