#pragma once

#include <filesystem>

#include "nix/util/canon-path.hh"
#include "nix/util/fun.hh"
#include "nix/util/hash.hh"
#include "nix/util/ref.hh"

namespace nix {

struct Sink;

/**
 * Note there is a decent chance this type soon goes away because the problem is solved another way.
 * See the discussion in https://github.com/NixOS/nix/pull/9985.
 */
enum class SymlinkResolution {
    /**
     * Resolve symlinks in the ancestors only.
     *
     * Only the last component of the result is possibly a symlink.
     */
    Ancestors,

    /**
     * Resolve symlinks fully, realpath(3)-style.
     *
     * No component of the result will be a symlink.
     */
    Full,
};

MakeError(SourceAccessorError, Error);
MakeError(FileNotFound, SourceAccessorError);
MakeError(NotASymlink, SourceAccessorError);
MakeError(NotADirectory, SourceAccessorError);
MakeError(NotARegularFile, SourceAccessorError);

/**
 * A content address a fetcher already knows for the tree its accessor's
 * root serves, together with the addressing scheme that produced it.
 *
 * The scheme has to travel *with* the id. A bare hash cannot say which
 * serialization it addresses, and the only use for a known tree id is to
 * turn it into a store path -- which requires ingesting the tree the same
 * way the id was computed. Two ids over different tree serializations are
 * different addresses even when they share a hash algorithm, so a consumer
 * that guessed would produce a store path that does not correspond to the
 * content. Carrying the family makes that mistake unrepresentable rather
 * than merely unlikely.
 */
struct KnownTreeRoot
{
    enum struct Family {
        /**
         * A Git tree object hash: the id of the root tree as Git
         * serializes it. `ContentAddressMethod::Raw::Git` ingests trees
         * that way, so this family can become a store path with no file
         * reads.
         */
        Git,

        /**
         * A root tree id from jj's own native (non-Git) object store, which
         * jj maintained incrementally while snapshotting. No nix ingestion
         * method serializes trees that way, so this family cannot become a
         * store path: a consumer must fall back to a method that reads the
         * tree, and may use the id only where an opaque content fingerprint
         * is wanted.
         */
        JjNative,
    };

    Family family;

    /** The id itself, in whatever hash algorithm the family uses. */
    Hash id;
};

/**
 * A read-only filesystem abstraction. This is used by the Nix
 * evaluator and elsewhere for accessing sources in various
 * filesystem-like entities (such as the real filesystem, tarballs or
 * Git repositories).
 */
struct SourceAccessor : std::enable_shared_from_this<SourceAccessor>
{
    const size_t number;

    std::string displayPrefix, displaySuffix;

    SourceAccessor();

    virtual ~SourceAccessor() {}

    /**
     * Return the contents of a file as a string.
     *
     * @note Unlike Unix, this method should *not* follow symlinks. Nix
     * by default wants to manipulate symlinks explicitly, and not
     * implicitly follow them, as they are frequently untrusted user data
     * and thus may point to arbitrary locations. Acting on the targets
     * targets of symlinks should only occasionally be done, and only
     * with care.
     */
    std::string readFile(const CanonPath & path);

    /**
     * Write the contents of a file as a sink. `sizeCallback` must be
     * called with the size of the file before any data is written to
     * the sink.
     *
     * @note Like the other `readFile`, this method should *not* follow
     * symlinks.
     *
     * @note subclasses of `SourceAccessor` need to implement at least
     * one of the `readFile()` variants.
     */
    virtual void readFile(const CanonPath & path, Sink & sink, fun<void(uint64_t)> sizeCallback = [](uint64_t size) {});

    virtual bool pathExists(const CanonPath & path);

    enum Type {
        tRegular,
        tSymlink,
        tDirectory,
        /**
          Any other node types that may be encountered on the file system, such as device nodes, sockets, named pipe,
          and possibly even more exotic things.

          Responsible for `"unknown"` from `builtins.readFileType "/dev/null"`.

          Unlike `DT_UNKNOWN`, this must not be used for deferring the lookup of types.
        */
        tChar,
        tBlock,
        tSocket,
        tFifo,
        tUnknown
    };

    struct Stat
    {
        Type type = tUnknown;

        /**
         * For regular files only: the size of the file. Not all
         * accessors return this since it may be too expensive to
         * compute.
         */
        std::optional<uint64_t> fileSize;

        /**
         * For regular files only: whether this is an executable.
         */
        bool isExecutable = false;

        /**
         * For regular files only: the position of the contents of this
         * file in the NAR. Only returned by NAR accessors.
         */
        std::optional<uint64_t> narOffset;

        bool isNotNARSerialisable();
        std::string typeString();
    };

    virtual Stat lstat(const CanonPath & path);

    virtual std::optional<Stat> maybeLstat(const CanonPath & path) = 0;

    typedef std::optional<Type> DirEntry;

    typedef std::map<std::string, DirEntry> DirEntries;

    /**
     * @note Like `readFile`, this method should *not* follow symlinks.
     */
    virtual DirEntries readDirectory(const CanonPath & path) = 0;

    virtual std::string readLink(const CanonPath & path) = 0;

    virtual void dumpPath(const CanonPath & path, Sink & sink, PathFilter & filter = defaultPathFilter);

    Hash
    hashPath(const CanonPath & path, PathFilter & filter = defaultPathFilter, HashAlgorithm ha = HashAlgorithm::SHA256);

    /**
     * Return a corresponding path in the root filesystem, if
     * possible. This is only possible for filesystems that are
     * materialized in the root filesystem.
     */
    virtual std::optional<std::filesystem::path> getPhysicalPath(const CanonPath & path)
    {
        return std::nullopt;
    }

    bool operator==(const SourceAccessor & x) const
    {
        return number == x.number;
    }

    auto operator<=>(const SourceAccessor & x) const
    {
        return number <=> x.number;
    }

    void setPathDisplay(std::string displayPrefix, std::string displaySuffix = "");

    /**
     * A name for the kind of thing this accessor is, stable across two runs of
     * the same evaluation.
     *
     * `displayPrefix` cannot serve. For the accessor answering for the local
     * filesystem it is deliberately empty, so that paths print as `/foo/bar`
     * rather than carrying a prefix, and that is the accessor behind every
     * ordinary `import`. Giving it a display to satisfy a trace would change
     * every error message that mentions a path.
     *
     * An empty return means this accessor cannot name itself, which the
     * read-set trace records as such rather than papering over: an unnamed
     * tree can only be matched across runs by the order it was first seen in,
     * and that is a guess indistinguishable from a measurement.
     *
     * Takes a path, and composite accessors delegate on it, for the same
     * reason `getFingerprint` does: which tree answers for a path is a
     * property of the path, not of the outermost accessor.
     */
    virtual std::string_view identityClass(const CanonPath & path)
    {
        return "";
    }

    virtual std::string showPath(const CanonPath & path);

    /**
     * Resolve any symlinks in `path` according to the given
     * resolution mode.
     *
     * @param mode might only be a temporary solution for this.
     * See the discussion in https://github.com/NixOS/nix/pull/9985.
     */
    CanonPath resolveSymlinks(const CanonPath & path, SymlinkResolution mode = SymlinkResolution::Full);

    /**
     * A string that uniquely represents the contents of this
     * accessor. This is used for caching lookups (see `fetchToStore()`).
     */
    std::optional<std::string> fingerprint;

    /**
     * If set, the content address that this accessor's root provably
     * serves, and the family it belongs to. Set only by fetchers that read
     * straight out of a content-addressed object store and know the served
     * tree is byte-identical to the named object: no omitted entries, no
     * content stored outside the tree.
     *
     * A consumer that wants to turn the id into a store path must test the
     * family against the specific one it can ingest -- never merely test
     * that an id is present, and never accept "some family addresses a
     * store path" as a proxy, because the ingestion method has to match the
     * serialization the id addresses. `Family::Git` plus
     * `ContentAddressMethod::Raw::Git` is that pairing today: it derives a
     * CA store path with zero file reads, where the NAR method's flat hash
     * re-reads the whole tree on every content change. Nix cannot name
     * `ContentAddressMethod` here (that is a libstore type), which is why
     * this header carries the family and each consumer states the pairing
     * itself.
     */
    std::optional<KnownTreeRoot> knownTreeRoot;

    /**
     * Return the fingerprint for `path`. This is usually the
     * fingerprint of the current accessor, but for composite
     * accessors (like `MountedSourceAccessor`), we want to return the
     * fingerprint of the "inner" accessor if the current one lacks a
     * fingerprint.
     *
     * So this method is intended to return the most-outer accessor
     * that has a fingerprint for `path`. It also returns the path that `path`
     * corresponds to in that accessor.
     *
     * For example: in a `MountedSourceAccessor` that has
     * `/nix/store/foo` mounted,
     * `getFingerprint("/nix/store/foo/bar")` will return the path
     * `/bar` and the fingerprint of the `/nix/store/foo` accessor.
     */
    virtual std::pair<CanonPath, std::optional<std::string>> getFingerprint(const CanonPath & path)
    {
        return {path, fingerprint};
    }

    /**
     * The last-modified time of this tree, if known. Fetchers that
     * know it set it (the Git fetcher uses the commit time) so that
     * composite accessors can surface it per subtree.
     */
    std::optional<time_t> lastModified;

    /**
     * Return the maximum last-modified time of the files in this
     * tree, if available.
     */
    virtual std::optional<time_t> getLastModified()
    {
        return lastModified;
    }

    /**
     * Return the last-modified time of the tree containing `path`.
     * Composite accessors (mounts) answer from the innermost mount,
     * so a Git submodule reports its own commit time rather than its
     * parent's; plain accessors answer path-independently.
     */
    virtual std::optional<time_t> getLastModified(const CanonPath & path)
    {
        return getLastModified();
    }

    /**
     * The revision this tree was fetched from, if known (the Git
     * fetcher sets it to the commit hash).
     */
    std::optional<Hash> rev;

    /**
     * Return the revision pinning the tree rooted at `path`. A rev
     * describes a whole tree, so plain accessors only answer at the
     * root; composite accessors answer at mount roots, so a Git
     * submodule reports its own gitlink commit while paths inside a
     * tree report nothing.
     */
    virtual std::optional<Hash> getRev(const CanonPath & path)
    {
        return path.isRoot() ? rev : std::nullopt;
    }

    /**
     * Invalidate any cached value the accessor may have for the specified path.
     */
    virtual void invalidateCache(const CanonPath & path) {}
};

/**
 * Return a source accessor that contains only an empty root directory.
 */
ref<SourceAccessor> makeEmptySourceAccessor();

/**
 * Exception thrown when accessing a filtered path (see
 * `FilteringSourceAccessor`).
 */
MakeError(RestrictedPathError, Error);

struct SymlinkNotAllowed final : public CloneableError<SymlinkNotAllowed, Error>
{
    CanonPath path;

    SymlinkNotAllowed(CanonPath path)
        : CloneableError("relative path '%s' points to a symlink, which is not allowed", path.rel())
        , path(std::move(path))
    {
    }

    template<typename... Args>
    SymlinkNotAllowed(CanonPath path, const std::string & fs, Args &&... args)
        : CloneableError(fs, std::forward<Args>(args)...)
        , path(std::move(path))
    {
    }
};

/**
 * Return an accessor for the root filesystem.
 */
ref<SourceAccessor> getFSSourceAccessor();

/**
 * Construct an accessor for the filesystem rooted at `root`. Note
 * that it is not possible to escape `root` by appending `..` path
 * elements, and that absolute symlinks are resolved relative to
 * `root`.
 */
ref<SourceAccessor> makeFSSourceAccessor(std::filesystem::path root, bool trackLastModified = false);

/**
 * Construct an accessor that presents a "union" view of a vector of
 * underlying accessors. Earlier accessors take precedence over later.
 */
ref<SourceAccessor> makeUnionSourceAccessor(std::vector<ref<SourceAccessor>> && accessors);

} // namespace nix
