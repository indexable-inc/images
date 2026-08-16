#include "nix/store/local-store.hh"
#include "nix/store/globals.hh"
#include "nix/util/signals.hh"
#include "nix/store/posix-fs-canonicalise.hh"
#include "nix/util/posix-source-accessor.hh"
#include "nix/util/file-system.hh"

#include <cstdlib>
#include <cstring>
#include <sys/types.h>
#include <sys/stat.h>
#include <unistd.h>
#include <errno.h>
#include <stdio.h>
#include <regex>

#include "store-config-private.hh"

#ifdef __APPLE__
#  include <sys/clonefile.h>
#  include <fcntl.h>
#endif

namespace nix {

#ifdef __APPLE__

/* The physical device offset of a file's first extent, or `std::nullopt` when
   the file has no data extents to speak of (empty, or decmpfs-compressed, in
   which case the data fork has been truncated). Two files that report the same
   offset share storage -- which is exactly what `clonefile` produces, and how
   we recognise work already done without keeping any persistent state. */
static std::optional<off_t> firstExtentOffset(const std::filesystem::path & path)
{
    AutoCloseFD fd{open(path.c_str(), O_RDONLY | O_CLOEXEC)};
    if (!fd)
        return std::nullopt;
    struct log2phys info;
    memset(&info, 0, sizeof(info));
    info.l2p_contigbytes = 1024 * 1024;
    info.l2p_devoffset = 0;
    if (fcntl(fd.get(), F_LOG2PHYS_EXT, &info) < 0)
        return std::nullopt;
    if (info.l2p_devoffset <= 0)
        return std::nullopt;
    return info.l2p_devoffset;
}

/** Whether the two paths already share their storage. */
static bool sharesExtents(const std::filesystem::path & a, const std::filesystem::path & b)
{
    auto oa = firstExtentOffset(a);
    if (!oa)
        return false;
    return oa == firstExtentOffset(b);
}

#endif

static void makeWritable(const std::filesystem::path & path)
{
    auto st = lstat(path);
    chmod(path, st.st_mode | S_IWUSR);
}

struct MakeReadOnly
{
    std::filesystem::path path;

    MakeReadOnly(std::filesystem::path path)
        : path(std::move(path))
    {
    }

    ~MakeReadOnly()
    {
        try {
            /* This will make the path read-only. */
            if (!path.empty())
                canonicaliseTimestampAndPermissions(path.string());
        } catch (...) {
            ignoreExceptionInDestructor();
        }
    }
};

LocalStore::InodeHash LocalStore::loadInodeHash()
{
    debug("loading hash inodes in memory");
    InodeHash inodeHash;

    AutoCloseDir dir(opendir(linksDir.string().c_str()));
    if (!dir)
        throw SysError("opening directory %1%", PathFmt(linksDir));

    struct dirent * dirent;
    while (errno = 0, dirent = readdir(dir.get())) { /* sic */
        checkInterrupt();
        // We don't care if we hit non-hash files, anything goes
        inodeHash.insert(dirent->d_ino);
    }
    if (errno)
        throw SysError("reading directory %1%", PathFmt(linksDir));

    printMsg(lvlTalkative, "loaded %1% hash inodes", inodeHash.size());

    return inodeHash;
}

Strings LocalStore::readDirectoryIgnoringInodes(const std::filesystem::path & path, const InodeHash & inodeHash)
{
    Strings names;

    AutoCloseDir dir(opendir(path.string().c_str()));
    if (!dir)
        throw SysError("opening directory %s", PathFmt(path));

    struct dirent * dirent;
    while (errno = 0, dirent = readdir(dir.get())) { /* sic */
        checkInterrupt();

        if (inodeHash.count(dirent->d_ino)) {
            debug("'%1%' is already linked", dirent->d_name);
            continue;
        }

        std::string name = dirent->d_name;
        if (name == "." || name == "..")
            continue;
        names.push_back(name);
    }
    if (errno)
        throw SysError("reading directory %s", PathFmt(path));

    return names;
}

void LocalStore::optimisePath_(
    Activity * act, OptimiseStats & stats, const std::filesystem::path & path, InodeHash & inodeHash, RepairFlag repair)
{
    checkInterrupt();

    auto st = lstat(path);

#ifdef __APPLE__
    /* HFS/macOS has some undocumented security feature disabling hardlinking for
       special files within .app dirs. Known affected paths include
       *.app/Contents/{PkgInfo,Resources/\*.lproj,_CodeSignature} and .DS_Store.
       See https://github.com/NixOS/nix/issues/1443 and
       https://github.com/NixOS/nix/pull/2230 for more discussion. */

    if (std::regex_search(path.string(), std::regex("\\.app/Contents/.+$"))) {
        debug("%s is not allowed to be linked in macOS", PathFmt(path));
        return;
    }
#endif

    if (S_ISDIR(st.st_mode)) {
        Strings names = readDirectoryIgnoringInodes(path, inodeHash);
        for (auto & i : names)
            optimisePath_(act, stats, path / i, inodeHash, repair);
        return;
    }

    /* We can hard link regular files and maybe symlinks. */
    if (!S_ISREG(st.st_mode)
#if CAN_LINK_SYMLINK
        && !S_ISLNK(st.st_mode)
#endif
    )
        return;

    /* Sometimes SNAFUs can cause files in the Nix store to be
       modified, in particular when running programs as root under
       NixOS (example: $fontconfig/var/cache being modified).  Skip
       those files.  FIXME: check the modification time. */
    if (S_ISREG(st.st_mode) && (st.st_mode & S_IWUSR)) {
        warn("skipping suspicious writable file '%s'", PathFmt(path));
        return;
    }

    /* This can still happen on top-level files. */
    if (st.st_nlink > 1 && inodeHash.count(st.st_ino)) {
        debug("%s is already linked, with %d other file(s)", PathFmt(path), st.st_nlink - 2);
        return;
    }

#ifdef __APPLE__
    /* Clones keep a link count of 1, so the check above cannot see them.
       Deduplicated-ness is instead a property of the storage: a file that
       already shares extents with the link-farm entry for its own hash is
       done. That check needs the hash, so it happens below, after hashing. */
    bool clone = settings.getLocalSettings().cloneStorePaths;
    if (clone && (st.st_flags & UF_COMPRESSED)) {
        /* A decmpfs-compressed file keeps its payload in the resource fork and
           has no data extents to share; cloning it would gain nothing and
           `sharesExtents` could not recognise the result. Compression has
           already claimed the space this would have saved. */
        debug("%s is compressed; leaving it to the compressor", PathFmt(path));
        return;
    }
#endif

    /* Hash the file.  Note that hashPath() returns the hash over the
       NAR serialisation, which includes the execute bit on the file.
       Thus, executable and non-executable files with the same
       contents *won't* be linked (which is good because otherwise the
       permissions would be screwed up).

       Also note that if `path' is a symlink, then we're hashing the
       contents of the symlink (i.e. the result of readlink()), not
       the contents of the target (which may not even exist). */
    Hash hash = ({
        hashPath(
            {make_ref<PosixSourceAccessor>(), CanonPath(path.string())},
            FileSerialisationMethod::NixArchive,
            HashAlgorithm::SHA256)
            .hash;
    });
    debug("%s has hash '%s'", PathFmt(path), hash.to_string(HashFormat::Nix32, true));

    /* Check if this is a known hash. */
    std::filesystem::path linkPath = std::filesystem::path{linksDir} / hash.to_string(HashFormat::Nix32, false);

    /* Maybe delete the link, if it has been corrupted. */
    if (std::filesystem::exists(std::filesystem::symlink_status(linkPath))) {
        auto stLink = lstat(linkPath);
        if (st.st_size != stLink.st_size || (repair && hash != ({
                                                           hashPath(
                                                               makeFSSourceAccessor(linkPath),
                                                               FileSerialisationMethod::NixArchive,
                                                               HashAlgorithm::SHA256)
                                                               .hash;
                                                       }))) {
            // XXX: Consider overwriting linkPath with our valid version.
            warn("removing corrupted link %s", PathFmt(linkPath));
            warn(
                "There may be more corrupted paths."
                "\nYou should run `nix-store --verify --check-contents --repair` to fix them all");
            std::filesystem::remove(linkPath);
        }
    }

    if (!std::filesystem::exists(std::filesystem::symlink_status(linkPath))) {
        /* Nope, create an entry in the links directory. */
        try {
#ifdef __APPLE__
            if (clone) {
                /* A clone costs no extra storage, so the link farm stays as
                   cheap as it is with hard links. Note that `nix-store --gc`
                   removes link-farm entries whose link count is 1, which is
                   every clone; that is harmless, because the sharing between
                   the store files themselves does not depend on the farm
                   entry, and the check above recognises it on the next pass. */
                if (clonefile(path.c_str(), linkPath.c_str(), 0) != 0)
                    throw SysError("cloning %1% to %2%", PathFmt(path), PathFmt(linkPath));
                inodeHash.insert(st.st_ino);
            } else
#endif
            {
                std::filesystem::create_hard_link(path, linkPath);
                inodeHash.insert(st.st_ino);
            }
        } catch (std::filesystem::filesystem_error & e) {
            if (e.code() == std::errc::file_exists) {
                /* Fall through if another process created ‘linkPath’ before
                   we did. */
            }

            else if (e.code() == std::errc::no_space_on_device) {
                /* On ext4, that probably means the directory index is
                   full.  When that happens, it's fine to ignore it: we
                   just effectively disable deduplication of this
                   file.
                   */
                printInfo("cannot link %s to '%s': %s", PathFmt(linkPath), PathFmt(path), e.code().message());
                return;
            }

            else
                throw SystemError(e.code(), "creating hard link from %1% to %2%", PathFmt(linkPath), PathFmt(path));
        }
    }

    /* Yes!  We've seen a file with the same contents.  Replace the
       current file with a hard link to that file. */
    auto stLink = lstat(linkPath);

    if (st.st_ino == stLink.st_ino) {
        debug("%1% is already linked to %2%", PathFmt(path), PathFmt(linkPath));
        return;
    }

#ifdef __APPLE__
    if (clone && sharesExtents(path, linkPath)) {
        debug("%1% already shares its storage with %2%", PathFmt(path), PathFmt(linkPath));
        return;
    }
#endif

    printMsg(lvlTalkative, "linking %1% to %2%", PathFmt(path), PathFmt(linkPath));

    /* Make the containing directory writable, but only if it's not
       the store itself (we don't want or need to mess with its
       permissions). */
    const auto dirOfPath = path.parent_path();
    bool mustToggle = dirOfPath != config->realStoreDir.get();
    if (mustToggle)
        makeWritable(dirOfPath);

    /* When we're done, make the directory read-only again and reset
       its timestamp back to 0. */
    MakeReadOnly makeReadOnly(mustToggle ? dirOfPath : std::filesystem::path{});

    std::filesystem::path tempLink = makeTempPath(config->realStoreDir.get(), ".tmp-link");

    try {
#ifdef __APPLE__
        if (clone) {
            if (clonefile(linkPath.c_str(), tempLink.c_str(), 0) != 0)
                throw SysError("cloning %1% to %2%", PathFmt(linkPath), PathFmt(tempLink));
            inodeHash.insert(st.st_ino);
        } else
#endif
        {
            std::filesystem::create_hard_link(linkPath, tempLink);
            inodeHash.insert(st.st_ino);
        }
    } catch (std::filesystem::filesystem_error & e) {
        if (e.code() == std::errc::too_many_links) {
            /* Too many links to the same file (>= 32000 on most file
               systems).  This is likely to happen with empty files.
               Just shrug and ignore. */
            if (st.st_size)
                printInfo("%1% has maximum number of links", PathFmt(linkPath));
            return;
        }
        throw SystemError(e.code(), "creating hard link from %1% to %2%", PathFmt(linkPath), PathFmt(tempLink));
    }

    /* Atomically replace the old file with the new hard link. */
    try {
        std::filesystem::rename(tempLink, path);
    } catch (std::filesystem::filesystem_error & e) {
        {
            std::error_code ec;
            remove(tempLink, ec); /* Clean up after ourselves. */
            if (ec)
                printError("unable to unlink %1%: %2%", PathFmt(tempLink), ec.message());
        }
        if (e.code() == std::errc::too_many_links) {
            /* Some filesystems generate too many links on the rename,
               rather than on the original link.  (Probably it
               temporarily increases the st_nlink field before
               decreasing it again.) */
            debug("%s has reached maximum number of links", PathFmt(linkPath));
            return;
        }
        throw SystemError(e.code(), "renaming %1% to %2%", PathFmt(tempLink), PathFmt(path));
    }

    stats.filesLinked++;
    stats.bytesFreed += st.st_size;

    if (act)
        act->result(
            resFileLinked,
            st.st_size
#ifndef _WIN32
            ,
            st.st_blocks
#endif
        );
}

void LocalStore::optimiseStore(OptimiseStats & stats)
{
    Activity act(*logger, actOptimiseStore);

    auto paths = queryAllValidPaths();
    InodeHash inodeHash = loadInodeHash();

    act.progress(0, paths.size());

    uint64_t done = 0;

    for (auto & i : paths) {
        addTempRoot(i);
        if (!isValidPath(i))
            continue; /* path was GC'ed, probably */
        {
            Activity act(*logger, lvlTalkative, actUnknown, fmt("optimising path '%s'", printStorePath(i)));
            optimisePath_(&act, stats, config->realStoreDir.get() / i.to_string(), inodeHash, NoRepair);
        }
        done++;
        act.progress(done, paths.size());
    }
}

void LocalStore::optimiseStore()
{
    OptimiseStats stats;

    optimiseStore(stats);

    printInfo("%s freed by hard-linking %d files", renderSize(stats.bytesFreed), stats.filesLinked);
}

void LocalStore::optimisePath(const std::filesystem::path & path, RepairFlag repair)
{
    OptimiseStats stats;
    InodeHash inodeHash;

    if (config->getLocalSettings().autoOptimiseStore)
        optimisePath_(nullptr, stats, path, inodeHash, repair);
}

} // namespace nix
