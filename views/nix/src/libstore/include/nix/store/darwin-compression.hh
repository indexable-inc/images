#pragma once
///@file

#ifdef __APPLE__

#  include <cstdint>
#  include <filesystem>

namespace nix {

/**
 * Statistics accumulated across compression attempts, for progress and
 * dry-run reporting by `nix store compress`.
 */
struct CompressionStats
{
    /** Regular files visited by `compressPathRecursively`. */
    uint64_t filesScanned = 0;
    /** Files compressed (or, in a dry run, that would be compressed). */
    uint64_t filesCompressed = 0;
    /** Files skipped because they already carry `UF_COMPRESSED`. */
    uint64_t filesAlreadyCompressed = 0;
    /** Files skipped because they are hard-linked (`st_nlink > 1`), i.e.
        deduplicated by the store optimiser. Compressing one name of such an
        inode would rewrite every other name too, so they are left alone. */
    uint64_t filesHardLinked = 0;
    /** Logical bytes of those skipped hard-linked files. */
    uint64_t bytesHardLinked = 0;
    /** Files that should have been compressible but failed (typically
        insufficient permission: compression needs to be run by the owner of
        the store, usually root). */
    uint64_t filesFailed = 0;
    /** Allocated bytes freed (measured after the fact for a real run,
        computed from the built payload for a dry run). */
    uint64_t bytesSaved = 0;
};

/**
 * Apply macOS transparent (decmpfs) compression to a regular file, using
 * LZFSE. The file's contents are unchanged as far as every reader is
 * concerned -- the kernel decompresses on read -- so this does not affect NAR
 * serialisation or store path hashes; only the on-disk footprint shrinks.
 *
 * A no-op (returning `false`) for anything that would not clearly benefit:
 * non-regular, empty, already-compressed or hard-linked files, and files whose
 * compressed payload would not free at least one allocation block.
 *
 * The round-trip is verified by reading the file back through the kernel
 * before the compression is accepted; on mismatch the original contents are
 * restored and `false` is returned.
 *
 * If `stats` is given, the outcome is recorded there. If `dryRun` is set,
 * the compressed payload is built and measured but nothing on disk is
 * touched; the return value then means "would have been compressed".
 */
bool compressPathIfWorthwhile(
    const std::filesystem::path & path, CompressionStats * stats = nullptr, bool dryRun = false);

/**
 * Recursively apply `compressPathIfWorthwhile` to every regular file under
 * `path` (which may itself be a regular file), accumulating into `stats`.
 * Used by `nix store compress` to sweep paths that are already in the store;
 * idempotent, since already-compressed files are recognised by their
 * `UF_COMPRESSED` flag and skipped cheaply.
 */
void compressPathRecursively(const std::filesystem::path & path, CompressionStats & stats, bool dryRun);

} // namespace nix

#endif
