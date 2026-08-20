#include "nix/fetchers/fetchers.hh"
#include "nix/util/file-system.hh"
#include "nix/util/fmt.hh"
#include "nix/util/os-string.hh"
#include "nix/util/processes.hh"
#include "nix/util/util.hh"
#include "nix/util/strings.hh"
#include "nix/util/executable-path.hh"
#include "nix/util/logging.hh"
#include "nix/util/source-accessor.hh"
#include "nix/fetchers/cache.hh"
#include "nix/fetchers/filtering-source-accessor.hh"
#include "nix/fetchers/git-utils.hh"
#include "nix/store/store-api.hh"
#include "nix/util/url-parts.hh"
#include "nix/fetchers/fetch-settings.hh"

#include <boost/unordered/unordered_flat_set.hpp>
#include <regex>

using namespace std::string_literals;

namespace nix::fetchers {

/* The template passed to `jj log` to extract metadata about the `@`
   (working-copy) commit. Fields are NUL-separated (so that bookmark names
   containing spaces are handled correctly):

     1. commit_id  -- the commit id, rendered in the backend's own hash: 40
        hexadecimal characters (SHA-1) on jj's Git backend, 64 (BLAKE3) on its
        native one
     2. committer timestamp, in seconds since the epoch
     3. whether the commit has conflicts ("1"/"0")
     4. the commit hashes of the parents, space-separated
     5..N. the names of the local bookmarks pointing at this commit, if any
*/
static constexpr std::string_view jjLogTemplate =
    R"(commit_id ++ "\0" ++ committer.timestamp().utc().format("%s") ++ "\0" ++ if(conflict, "1", "0") ++ "\0" ++ parents.map(|c| c.commit_id()).join(" ") ++ "\0" ++ local_bookmarks.map(|b| b.name()).join("\0"))";

static RunOptions jjOptions(const std::filesystem::path & repoDir, OsStrings args, bool ignoreWorkingCopy)
{
    OsStrings allArgs{
        // Pin an identity so that snapshotting the working copy never fails or
        // prompts in environments without a jj/user config. This does not change
        // the author of the existing `@` commit (it only matters when a *new*
        // commit is created), so it is safe for the read-only operations we do.
        OS_STR("--config"),
        OS_STR("user.name=nix"),
        OS_STR("--config"),
        OS_STR("user.email=nix@localhost"),
        // Deterministic, non-interactive output.
        OS_STR("--color"),
        OS_STR("never"),
        OS_STR("--config"),
        OS_STR("ui.paginate=never"),
    };
    if (ignoreWorkingCopy)
        allArgs.push_back(OS_STR("--ignore-working-copy"));
    for (auto & arg : args)
        allArgs.push_back(std::move(arg));

    return {
        .program = "jj",
        .lookupPath = true,
        .args = std::move(allArgs),
        // jj prints file paths relative to the working directory, so run it from
        // the repository root to get root-relative paths.
        .chdir = repoDir,
    };
}

// runProgram wrapper that uses jjOptions instead of stock RunOptions.
static std::string runJj(const std::filesystem::path & repoDir, OsStrings args, bool ignoreWorkingCopy = false)
{
    auto res = runProgram(jjOptions(repoDir, std::move(args), ignoreWorkingCopy));

    if (!statusOk(res.first))
        throw ExecError(res.first, "jj %1%", statusToString(res.first));

    return res.second;
}

struct JjInputScheme : InputScheme
{
    std::optional<Input> inputFromURL(const Settings & settings, const ParsedURL & url, bool requireTree) const override
    {
        // We currently only support local Jujutsu working copies. Remote jj
        // repositories (jj+https, jj+ssh, ...) would require cloning, which is
        // not yet implemented.
        if (url.scheme != "jj+file")
            return {};

        auto url2(url);
        url2.scheme = std::string(url2.scheme, 3);
        url2.query.clear();

        Attrs attrs;
        attrs.emplace("type", "jj");

        for (auto & [name, value] : url.query) {
            if (name == "rev" || name == "ref")
                attrs.emplace(name, value);
            // `dir` is meaningful to the flake layer (it selects a subdirectory),
            // not to this fetcher, which always copies the whole working copy. Drop
            // it so it doesn't leak into the stored URL (cf. git.cc).
            else if (name == "dir")
                continue;
            else
                url2.query.emplace(name, value);
        }

        attrs.emplace("url", url2.to_string());

        return inputFromAttrs(settings, attrs);
    }

    std::string_view schemeName() const override
    {
        return "jj";
    }

    std::string schemeDescription() const override
    {
        return "a Jujutsu (jj) working copy";
    }

    const std::map<std::string, AttributeInfo> & allowedAttrs() const override
    {
        static const std::map<std::string, AttributeInfo> attrs = {
            {
                "url",
                {},
            },
            {
                "ref",
                {},
            },
            {
                "rev",
                {},
            },
            {
                "revCount",
                {},
            },
            {
                "lastModified",
                {},
            },
            {
                "narHash",
                {},
            },
            {
                "name",
                {},
            },
        };
        return attrs;
    }

    std::optional<Input> inputFromAttrs(const Settings & settings, const Attrs & attrs) const override
    {
        parseURL(getStrAttr(attrs, "url"));

        if (auto ref = maybeGetStrAttr(attrs, "ref")) {
            if (!std::regex_match(*ref, refRegex))
                throw BadURL("invalid Jujutsu bookmark name '%s'", *ref);
        }

        Input input{};
        input.attrs = attrs;
        return input;
    }

    ParsedURL toURL(const Input & input) const override
    {
        auto url = parseURL(getStrAttr(input.attrs, "url"));
        url.scheme = "jj+" + url.scheme;
        if (auto rev = input.getRev())
            url.query.insert_or_assign("rev", rev->gitRev());
        if (auto ref = input.getRef())
            url.query.insert_or_assign("ref", *ref);
        return url;
    }

    Input applyOverrides(const Input & input, std::optional<std::string> ref, std::optional<Hash> rev) const override
    {
        auto res(input);
        if (rev)
            res.attrs.insert_or_assign("rev", rev->gitRev());
        if (ref)
            res.attrs.insert_or_assign("ref", *ref);
        return res;
    }

    std::optional<std::filesystem::path> getSourcePath(const Input & input) const override
    {
        auto url = parseURL(getStrAttr(input.attrs, "url"));
        if (url.scheme == "file" && !input.getRef() && !input.getRev())
            return urlPathToPath(url.path);
        return {};
    }

    void putFile(
        const Input & input,
        const CanonPath & path,
        std::string_view contents,
        std::optional<std::string> commitMsg) const override
    {
        auto repoPath = getSourcePath(input);
        if (!repoPath)
            throw Error(
                "cannot commit '%s' to Jujutsu repository '%s' because it's not a working tree",
                path,
                input.to_string());

        writeFile(*repoPath / path.rel(), contents);

        // Unlike Git and Mercurial, Jujutsu automatically tracks new files when
        // it next snapshots the working copy, so there is nothing to "add". The
        // change becomes part of the `@` commit on the next jj invocation.
    }

    std::filesystem::path getActualPath(const Input & input) const
    {
        auto url = parseURL(getStrAttr(input.attrs, "url"));
        if (url.scheme != "file")
            throw Error(
                "Jujutsu input '%s' is not a local working copy; only file:// URLs are supported", input.to_string());
        return absPath(urlPathToPath(url.path));
    }

    static MakeNotAllowedError makeNotAllowedError(std::filesystem::path repoPath)
    {
        return [repoPath{std::move(repoPath)}](const CanonPath & path) -> RestrictedPathError {
            if (pathExists(repoPath / path.rel()))
                return RestrictedPathError(
                    "Path '%1%' in the repository %2% is not tracked by Jujutsu.\n"
                    "\n"
                    "To make it visible to Nix, run:\n"
                    "\n"
                    "jj file track %1%",
                    path.rel(),
                    PathFmt(repoPath));
            else
                return RestrictedPathError(
                    "Path '%s' does not exist in Jujutsu repository %s.", path.rel(), PathFmt(repoPath));
        };
    }

    struct Metadata
    {
        Hash rev;
        uint64_t lastModified;
        bool hasConflict;
        /* The parents of the revision. Working-copy churn rewrites `@` in
           place -- new commit hash, same parents -- so the parents are what
           stays stable across edits (see `getRevCount`). */
        std::vector<Hash> parents;
        /* The names of the local bookmarks pointing at the revision, if any. */
        std::vector<std::string> bookmarks;
    };

    Metadata
    readMetadata(const std::filesystem::path & repoPath, const std::string & revset, bool ignoreWorkingCopy) const
    {
        auto output = runJj(
            repoPath,
            {OS_STR("log"),
             OS_STR("-r"),
             string_to_os_string(revset),
             OS_STR("--no-graph"),
             OS_STR("-T"),
             string_to_os_string(std::string(jjLogTemplate))},
            ignoreWorkingCopy);

        /* `splitString`, not `tokenizeString`: the latter drops empty
           fields, and the parents field is positional (empty only for the
           root commit itself). */
        auto fields = splitString<std::vector<std::string>>(output, "\0"s);
        if (fields.size() < 4)
            throw Error("unexpected output from 'jj log' for repository %s", PathFmt(repoPath));

        /* `parseRev`, not a fixed algorithm: the template above renders ids in
           whatever hash the backend uses, so the length decides. Reading a
           native-backend repo hard-failed here before, one parse short of
           working. */
        std::vector<Hash> parents;
        for (auto & p : tokenizeString<std::vector<std::string>>(chomp(fields[3]), " "))
            parents.push_back(parseRev(p));

        std::vector<std::string> bookmarks;
        for (auto it = fields.begin() + 4; it != fields.end(); ++it)
            if (auto bookmark = chomp(*it); !bookmark.empty())
                bookmarks.push_back(std::move(bookmark));

        return Metadata{
            .rev = parseRev(chomp(fields[0])),
            .lastModified = string2Int<uint64_t>(chomp(fields[1])).value_or(0),
            .hasConflict = chomp(fields[2]) == "1",
            .parents = std::move(parents),
            .bookmarks = std::move(bookmarks),
        };
    }

    /* Count the number of ancestors of the revision (including itself),
       caching the result by commit hash since this is independent of the
       working-copy state.

       The cache key changes on every edit: jj snapshots the working copy by
       rewriting `@` -- new commit hash, same parents -- so a by-rev cache
       alone re-walked the whole DAG per edit (~5 s of every post-edit
       evaluation on a 158k-commit repo, measured). But |::rev| for a
       single-parent rev is |::parent| + 1 (the two sets differ exactly by
       `rev` itself), and the parents survive the rewrite, so a cached count
       for the parent answers in O(1). Seeding works the same way in
       reverse: one full walk also stores the parent's count, and every
       later rewrite of `@` on those parents is a hit. A merge `@` falls
       back to the full walk: ancestor counts do not add up across
       parents. */
    uint64_t getRevCount(ref<Cache> cache, const std::filesystem::path & repoPath, const Metadata & meta) const
    {
        auto keyFor = [](const Hash & rev) {
            return Cache::Key{"jjRevCount", {{"rev", rev.gitRev()}}};
        };

        auto key = keyFor(meta.rev);

        if (auto revCountAttrs = cache->lookup(key))
            return getIntAttr(*revCountAttrs, "revCount");

        std::optional<uint64_t> revCount;

        if (meta.parents.size() == 1)
            if (auto parentAttrs = cache->lookup(keyFor(meta.parents.front())))
                revCount = getIntAttr(*parentAttrs, "revCount") + 1;

        if (!revCount) {
            Activity act(
                *logger, lvlChatty, actUnknown, fmt("getting Jujutsu revision count of '%s'", PathFmt(repoPath)));

            /* Walk the backing Git repository where there is one: GitRepo's
               parallel in-process walk beats streaming one template line per
               commit out of a `jj log` subprocess. `+ 1` because jj's
               `::rev` also counts jj's virtual root commit (the all-zeros
               parent of every rootless commit), which has no Git
               counterpart; the two walks must agree or the attribute would
               flap by one with cache state. Any failure (say, an ancestor
               missing from the odb) falls back to the jj walk below, which
               defines the semantics. */
            if (auto gitStore = gitStorePath(repoPath)) {
                try {
                    auto repo = GitRepo::openRepo(*gitStore, {});
                    if (repo->hasObject(meta.rev))
                        revCount = repo->getRevCount(meta.rev) + 1;
                } catch (Error & e) {
                    debug(
                        "failed to count Jujutsu revision '%s' via Git, falling back to jj: %s",
                        meta.rev.gitRev(),
                        e.what());
                }
            }

            if (!revCount) {
                auto output = runJj(
                    repoPath,
                    {OS_STR("log"),
                     OS_STR("-r"),
                     string_to_os_string("::" + meta.rev.gitRev()),
                     OS_STR("--no-graph"),
                     OS_STR("-T"),
                     OS_STR("\"x\\n\"")},
                    /*ignoreWorkingCopy=*/true);

                uint64_t n = 0;
                for (auto & line : tokenizeString<std::vector<std::string>>(output, "\n"))
                    if (!line.empty())
                        n++;
                revCount = n;
            }

            if (meta.parents.size() == 1 && *revCount > 0)
                cache->upsert(keyFor(meta.parents.front()), Attrs{{"revCount", *revCount - 1}});
        }

        cache->upsert(key, Attrs{{"revCount", *revCount}});

        return *revCount;
    }

    void setAttrs(
        const Settings & settings, Input & input, const std::filesystem::path & repoPath, const Metadata & meta) const
    {
        auto rev = meta.rev;
        input.attrs.insert_or_assign("rev", rev.gitRev());
        input.attrs.insert_or_assign("lastModified", meta.lastModified);
        input.attrs.insert_or_assign("revCount", getRevCount(settings.getCache(), repoPath, meta));
        // Expose a bookmark as `ref` when it unambiguously names a single one and
        // the caller didn't request a specific ref.
        if (!input.getRef() && meta.bookmarks.size() == 1)
            input.attrs.insert_or_assign("ref", meta.bookmarks[0]);
    }

    /* Build a jj fileset string literal that matches exactly `path`, so that
       paths containing characters with meaning in fileset expressions (spaces,
       `:`, parentheses, ...) are passed through verbatim. jj string literals use
       C-style backslash escaping. */
    static OsString jjFileset(const std::string & path)
    {
        std::string escaped;
        for (auto c : path) {
            switch (c) {
            case '"':
                escaped += "\\\"";
                break;
            case '\\':
                escaped += "\\\\";
                break;
            case '\n':
                escaped += "\\n";
                break;
            case '\r':
                escaped += "\\r";
                break;
            case '\t':
                escaped += "\\t";
                break;
            default:
                escaped += c;
            }
        }
        return string_to_os_string("file:\"" + escaped + "\"");
    }

    /* `jj file show` refuses to read a symlink, so recover its target from a
       git-format diff against the empty tree, where a symlink appears as a
       `new file mode 120000` blob whose content is the target. */
    std::string
    readSymlinkTarget(const std::filesystem::path & repoPath, const std::string & rev, const std::string & path) const
    {
        auto diff = runJj(
            repoPath,
            {OS_STR("diff"),
             OS_STR("--from"),
             OS_STR("root()"),
             OS_STR("--to"),
             string_to_os_string(rev),
             OS_STR("--git"),
             jjFileset(path)},
            /*ignoreWorkingCopy=*/true);

        /* Only the body of the hunk (after the `@@ ... @@` line) holds the
           target. We must not look for added lines before it, because the
           `+++ b/<path>` diff header — and a target that itself starts with `+`
           — would otherwise be confused with content. */
        std::string target;
        bool inHunk = false;
        for (auto & line : splitString<std::vector<std::string>>(diff, "\n")) {
            if (!inHunk) {
                if (hasPrefix(line, "@@"))
                    inHunk = true;
                continue;
            }
            // Added lines hold the target; `\` introduces the "No newline at end
            // of file" marker, which we ignore.
            if (hasPrefix(line, "+")) {
                if (!target.empty())
                    target += "\n";
                target += line.substr(1);
            }
        }
        if (target.empty())
            throw Error("could not determine the target of symlink '%s' at revision %s", path, rev);
        return target;
    }

    /* Materialise the tree of `rev` into `destDir` with a single `jj`
       invocation, on the repos whose backend offers one.

       `jj ix export` walks the tree in-process and fetches contents in
       `object.get_batch` round trips of 64 objects, four in flight. That
       replaces the per-file route below, whose cost is not the reading
       but the *process*: one `jj` start measures 14.5 ms on this machine
       against a local store, so a 174,845-file tree spent ~42 min in
       `fork`/`exec` before accounting for a single byte. Against a
       remote store each invocation also paid its own connection setup,
       measured at 630 ms/file over a 124 ms-RTT link, and the resulting
       handshake storm was heavy enough to make the store server itself
       time out mid-evaluation.

       Only the ix backend takes this route. That is the precise
       condition and not a proxy for one: those repos are exactly the
       ones a stock `jj` cannot read at all, so a `jj` that can read them
       is a `jj` that has this subcommand. A conflicted revision in a
       Git-backed repo -- the other caller of the per-file route -- keeps
       working with whatever `jj` is on PATH.

       Returns the root tree id the export reported, when it reported
       one. It is a blake3 id over jj's own tree serialization, so nix
       cannot ingest it into a store path, but it is a true content
       address for the tree and two commits over one tree share it. */
    std::optional<std::string> exportRevBulk(
        const std::filesystem::path & repoPath, const std::string & rev, const std::filesystem::path & destDir) const
    {
        auto output = runJj(
            repoPath,
            {OS_STR("ix"),
             OS_STR("export"),
             OS_STR("-r"),
             string_to_os_string(rev),
             string_to_os_string(destDir.string())},
            /*ignoreWorkingCopy=*/true);

        /* `key value` lines. Only `tree` is read back; the counts are for
           a human reading a log. An unrecognised key is ignored rather
           than rejected so that adding one later is not a breaking
           change. */
        std::optional<std::string> treeId;
        for (auto & line : splitString<std::vector<std::string>>(output, "\n")) {
            auto trimmed = chomp(line);
            if (hasPrefix(trimmed, "tree "))
                treeId = trimmed.substr(5);
        }
        return treeId;
    }

    /* Materialise the tree of `rev` into `destDir` one file at a time.
       jj has no `git archive` / `hg archive` equivalent, so we
       reconstruct it: the file template gives the type and executable
       bit, `jj file show` gives file contents (binary-safe), and
       `readSymlinkTarget` recovers symlink targets.

       This is one `jj` invocation per file. What is left on it is the
       case `exportRevBulk` above cannot take: a revision carrying a
       conflict in a repo whose backend is not ix, where the materialised
       content is not in Git objects at all and the `jj` on PATH may be
       any `jj`. Such a tree is small and rare, which is what makes the
       per-file cost affordable here and ruinous there. */
    void exportRevPerFile(
        const std::filesystem::path & repoPath, const std::string & rev, const std::filesystem::path & destDir) const
    {
        auto listing = runJj(
            repoPath,
            {OS_STR("file"),
             OS_STR("list"),
             OS_STR("-r"),
             string_to_os_string(rev),
             OS_STR("-T"),
             OS_STR(R"(path ++ "\0" ++ file_type ++ "\0" ++ if(executable, "x", "-") ++ "\0")")},
            /*ignoreWorkingCopy=*/true);

        auto tokens = tokenizeString<std::vector<std::string>>(listing, "\0"s);
        if (tokens.size() % 3 != 0)
            throw Error("unexpected output from 'jj file list' for revision %s", rev);

        for (size_t i = 0; i + 3 <= tokens.size(); i += 3) {
            auto & path = tokens[i];
            auto & type = tokens[i + 1];
            auto executable = tokens[i + 2] == "x";

            auto dest = destDir / CanonPath(path).rel();
            createDirs(dest.parent_path());

            // `file_type` is one of file, symlink, conflict, git-submodule (or
            // tree, which `jj file list` never yields). A conflicted file is
            // materialised by `jj file show` as conflict markers, matching what
            // the working copy contains.
            if (type == "symlink")
                createSymlink(readSymlinkTarget(repoPath, rev, path), dest);
            else if (type == "file" || type == "conflict") {
                writeFile(
                    dest,
                    runJj(
                        repoPath,
                        {OS_STR("file"), OS_STR("show"), OS_STR("-r"), string_to_os_string(rev), jjFileset(path)},
                        /*ignoreWorkingCopy=*/true));
                if (executable)
                    nix::chmod(dest, 0755);
            } else
                throw Error(
                    "cannot fetch Jujutsu revision %s: file '%s' has unsupported type '%s' "
                    "(Git submodules are not supported)",
                    rev,
                    path,
                    type);
        }
    }

    /* The Git repository backing a jj repo's object store, if it has one.
       jj's default backend is Git, and the working copy snapshot it takes
       below is an ordinary Git commit in that repository, so `@`'s content is
       readable from Git objects rather than from the files on disk. A repo on
       jj's native backend has no such path and keeps reading the filesystem.

       `.jj/repo` is a directory in the primary workspace and a file naming the
       real repo directory in a secondary one; `store/git_target` is then a
       path relative to `store/`. Both layouts occur: a colocated repo points
       at the workspace's own `.git`, a plain `jj git init` at
       `.jj/repo/store/git`. */
    static std::optional<std::filesystem::path> storeDirOf(const std::filesystem::path & repoPath)
    {
        auto repoDir = repoPath / ".jj" / "repo";
        if (!std::filesystem::is_directory(repoDir)) {
            if (!pathExists(repoDir))
                return std::nullopt;
            auto named = std::filesystem::path(chomp(readFile(repoDir)));
            repoDir = named.is_absolute() ? named : repoPath / ".jj" / named;
        }
        return repoDir / "store";
    }

    /* The backend name recorded in `store/type`, if the repo has one.
       `git` and `ix` are the two that matter here; jj writes others. */
    static std::optional<std::string> storeType(const std::filesystem::path & repoPath)
    {
        auto storeDir = storeDirOf(repoPath);
        if (!storeDir)
            return std::nullopt;
        auto typeFile = *storeDir / "type";
        if (!pathExists(typeFile))
            return std::nullopt;
        return chomp(readFile(typeFile));
    }

    /* Whether this repo's objects live in an ix jj store (ADR 0001)
       rather than in Git or on disk.

       Used to pick the export route below, and it is the *precise*
       condition rather than a proxy: a repo on that backend can only be
       read by a `jj` that has the backend compiled in, so on such a repo
       `jj ix export` is available by the same fact that makes the repo
       readable at all. Probing for the subcommand instead would ask a
       weaker question and answer it more slowly. */
    static bool isIxStore(const std::filesystem::path & repoPath)
    {
        auto type = storeType(repoPath);
        return type && *type == "ix";
    }

    static std::optional<std::filesystem::path> gitStorePath(const std::filesystem::path & repoPath)
    {
        auto storeDirOpt = storeDirOf(repoPath);
        if (!storeDirOpt)
            return std::nullopt;
        auto storeDir = *storeDirOpt;
        auto typeFile = storeDir / "type";
        auto targetFile = storeDir / "git_target";
        if (!pathExists(typeFile) || chomp(readFile(typeFile)) != "git" || !pathExists(targetFile))
            return std::nullopt;

        auto target = storeDir / chomp(readFile(targetFile));
        if (!pathExists(target))
            return std::nullopt;
        return target;
    }

    std::pair<ref<SourceAccessor>, Input> getAccessorFromWorkdir(
        const Settings & settings, Store & store, const std::filesystem::path & repoPath, Input input) const
    {
        /* Snapshot the working copy and read metadata about the `@` commit. We
           deliberately do *not* pass `--ignore-working-copy` here: snapshotting
           is what makes `@` reflect the current on-disk state (jj has no notion
           of a separate "dirty" state). */
        auto meta = readMetadata(repoPath, "@", /*ignoreWorkingCopy=*/false);

        if (meta.hasConflict)
            warn(
                "Jujutsu working copy %s has unresolved conflicts; conflict markers will be included",
                PathFmt(repoPath));

        /* Read the snapshot jj has just taken, rather than the files it was
           taken from. The two agree only until something writes to the working
           copy: the allow list built below constrains which paths an
           evaluation may read, never which version of them, so a write between
           the `jj file list` and the reads it guards puts two states into one
           evaluation. Reading `@`'s tree removes that window rather than
           narrowing it, which is also what makes the input safe to mount
           lazily (indexable-inc/index#3749).

           A conflict keeps the filesystem read: jj does not store a conflicted
           file's materialised content in the Git tree, and the markers the
           warning above promises are on disk. */

        /* Enumerate the files tracked in the `@` commit. The snapshot has already
           happened above, so we can skip it here. We NUL-separate the paths so
           that filenames containing newlines are handled correctly.

           `file_type` comes along because the entries below become allow-list
           PREFIXES, and a Git submodule is one entry naming a directory. Allowing
           that prefix would admit every file physically under the submodule
           working tree, including its own `.git` pointer file, so a colocated
           repo with a submodule produced a tree that no `git+file` fetch can
           produce: submodule content without `?submodules=1`, plus a dangling
           `gitdir:` pointer baked into the store. */
        /* Pinned to the commit `readMetadata` resolved, not left to re-resolve
           `@`. Nothing stops `@` moving between the two calls: any jj command
           run against this repo snapshots the working copy, including a second
           `nix` evaluation fetching the same input. Listing one commit while
           serving another would decide `skipped` from a tree that is not the
           one below, which is the same "one evaluation, two states" the rest
           of this function exists to prevent. */
        auto fileList = runJj(
            repoPath,
            {OS_STR("file"),
             OS_STR("list"),
             OS_STR("-r"),
             string_to_os_string(meta.rev.gitRev()),
             OS_STR("-T"),
             OS_STR(R"(path ++ "\0" ++ file_type ++ "\0")")},
            /*ignoreWorkingCopy=*/true);

        auto tokens = tokenizeString<std::vector<std::string>>(fileList, "\0"s);
        if (tokens.size() % 2 != 0)
            throw Error("unexpected output from 'jj file list' in %s", PathFmt(repoPath));

        /* Exact paths, not prefixes. `CanonPath::isAllowed` deliberately allows
           a path when EITHER it is a parent of something allowed (so a walk can
           descend to a tracked file) OR something allowed is a parent of it. The
           second half is what a file list must not lean on: it turns any listed
           entry that happens to be a directory into a licence for everything
           physically beneath it, tracked or not. So the ancestors are listed
           explicitly, which buys the descent without the licence. */
        boost::unordered_flat_set<CanonPath> allowed;
        allowed.insert(CanonPath::root);

        /* Only the types that name a single non-directory object. Taken as an
           allow-list rather than a deny-list of `git-submodule`, because
           `describe_file_type` in jj's commit_templater.rs is the whole
           vocabulary today (file, symlink, tree, git-submodule, conflict, and
           "" for absent) and a later jj may add to it. An unknown type is then
           skipped and reported rather than silently admitted.

           `tree` does not appear in practice: `jj file list` iterates
           `tree.entries_matching`, which descends subtrees and yields leaves. A
           submodule is a leaf jj cannot descend into, which is how it reaches
           this list at all. */
        std::map<std::string, size_t> skipped;
        for (size_t i = 0; i + 2 <= tokens.size(); i += 2) {
            auto & path = tokens[i];
            auto & type = tokens[i + 1];
            if (path.empty())
                continue;
            // Defensive: ignore anything outside the repository root.
            if (hasPrefix(path, "../"))
                continue;
            if (type != "file" && type != "symlink" && type != "conflict") {
                skipped[type]++;
                continue;
            }
            auto file = CanonPath(path);
            allowed.insert(file);
            for (auto parent = file; !parent.isRoot();) {
                parent.pop();
                allowed.insert(parent);
            }
        }
        for (auto & [type, count] : skipped)
            /* A `git+file` input without `submodules=1` renders a submodule
               absent, and this matches that. Saying so beats a tree that
               quietly lacks a directory the user can see on disk. */
            warn(
                "Jujutsu working copy %s has %d path(s) of type '%s', whose contents are not included in the flake source",
                PathFmt(repoPath),
                count,
                type == "" ? "absent" : type);

        /* Everything above agreed with `@`'s tree, so read the tree instead of
           the files it was made from. The Git accessor can omit gitlinks, which
           renders `git-submodule` absent as promised above. A conflict or any
           other skipped type stays on the fallback because Git cannot render
           the working-copy form the warning describes. */
        auto canReadGitTree = skipped.empty() || (skipped.size() == 1 && skipped.contains("git-submodule"));
        if (!meta.hasConflict && canReadGitTree) {
            if (auto gitStore = gitStorePath(repoPath)) {
                auto repo = GitRepo::openRepo(*gitStore, {});
                setAttrs(settings, input, repoPath, meta);
                auto accessor = repo->getAccessor(meta.rev, {.omitGitlinks = true}, "«" + input.to_string() + "»");
                /* When nothing was omitted from the served tree -- no
                   gitlinks (`skipped` is empty, so `jj file list` saw no
                   submodule leaf) and no legacy out-of-tree conflict
                   storage ("jj:trees" header) -- this accessor serves
                   byte-for-byte the tree object `meta.rev` names, so its
                   git hash is known without reading a single file: jj
                   maintained it incrementally while snapshotting. Announce
                   it; under the `git-hashing` experimental feature the
                   mount's store path then follows from it directly,
                   instead of flat-NAR-hashing the whole tree on every
                   content edit (fetch-to-store.cc, paths.cc). */
                if (skipped.empty() && !repo->hasCommitExtraHeader(meta.rev, "jj:trees"))
                    accessor->knownTreeRoot = KnownTreeRoot{
                        .family = KnownTreeRoot::Family::Git,
                        .id = repo->getTreeHash(meta.rev),
                    };
                return {accessor, std::move(input)};
            }
        }

        ref<SourceAccessor> accessor = AllowListSourceAccessor::create(
                                           makeFSSourceAccessor(repoPath),
                                           /*allowedPrefixes=*/{},
                                           std::move(allowed),
                                           makeNotAllowedError(repoPath))
                                           .cast<SourceAccessor>();

        setAttrs(settings, input, repoPath, meta);

        accessor->setPathDisplay("«" + input.to_string() + "»");

        return {accessor, std::move(input)};
    }

    /* Fetch an explicitly-requested revision or bookmark: read it out of the
       Git store where there is one, and otherwise materialise its tree into the
       store a file at a time. */
    std::pair<ref<SourceAccessor>, Input> getAccessorFromRev(
        const Settings & settings, Store & store, const std::filesystem::path & repoPath, Input input) const
    {
        auto revset = input.getRev() ? input.getRev()->gitRev() : *input.getRef();

        auto meta = readMetadata(repoPath, revset, /*ignoreWorkingCopy=*/true);

        if (meta.hasConflict)
            warn("Jujutsu revision '%s' has unresolved conflicts; conflict markers will be included", revset);

        /* Read the revision out of the Git repository backing the store, rather
           than reconstructing it a file at a time. A rev is immutable and
           content-addressed, so the export below produced the same store path
           on every fetch and nothing remembered it had: measured at 16 ms per
           file, 238s for a 14,749-file repo, paid again on each evaluation
           (ENG-11699). Reading the tree removes the work instead of caching it,
           so a revision nobody has fetched before is cheap too.

           `fetchJj.sh` already pins this to the same bytes: it asserts a by-rev
           fetch and a working-copy fetch of the same commit yield one store
           path, over a tree carrying symlinks, an executable bit and ignored
           files, and the working copy is read from Git the same way.

           A conflict keeps the export: jj does not store a conflicted file's
           materialised content in the Git tree, and the markers the warning
           above promises come from `jj file show`. */
        if (!meta.hasConflict) {
            if (auto gitStore = gitStorePath(repoPath)) {
                auto repo = GitRepo::openRepo(*gitStore, {});
                setAttrs(settings, input, repoPath, meta);
                auto accessor = repo->getAccessor(meta.rev, {.omitGitlinks = true}, "«" + input.to_string() + "»");
                return {accessor, std::move(input)};
            }
        }

        auto tmpDir = createTempDir();
        AutoDelete delTmpDir(tmpDir, true);

        if (isIxStore(repoPath))
            exportRevBulk(repoPath, meta.rev.gitRev(), tmpDir);
        else
            exportRevPerFile(repoPath, meta.rev.gitRev(), tmpDir);

        auto storePath = store.addToStore(input.getName(), {makeFSSourceAccessor(tmpDir), CanonPath::root});
        auto accessor = store.requireStoreObjectAccessor(storePath);

        setAttrs(settings, input, repoPath, meta);

        accessor->setPathDisplay("«" + input.to_string() + "»");

        return {accessor, std::move(input)};
    }

    std::pair<ref<SourceAccessor>, Input>
    getAccessor(const Settings & settings, Store & store, const Input & _input) const override
    {
        Input input(_input);

        auto repoPath = getActualPath(input);

        if (!pathExists(repoPath / ".jj"))
            throw Error("%s is not a Jujutsu repository (it has no '.jj' directory)", PathFmt(repoPath));

        /* Flake references to a local path are routed here automatically when a
           `.jj` directory is found (see `flakeref.cc`), so give a clear error if
           the `jj` command isn't available rather than a cryptic exec failure. */
        if (!ExecutablePath::load().findName(OS_STR("jj")))
            throw Error(
                "the 'jj' command is required to evaluate a flake in the Jujutsu working copy %s, "
                "but it was not found in PATH.\n"
                "\n"
                "Install Jujutsu (https://jj-vcs.github.io/), or use a colocated Git repository "
                "('jj git init --colocate').",
                PathFmt(repoPath));

        return input.getRev() || input.getRef() ? getAccessorFromRev(settings, store, repoPath, std::move(input))
                                                : getAccessorFromWorkdir(settings, store, repoPath, std::move(input));
    }

    bool isLocked(const Settings & settings, const Input & input) const override
    {
        return (bool) input.getRev();
    }

    std::optional<std::string> getFingerprint(Store & store, const Input & input) const override
    {
        auto rev = input.getRev();
        if (!rev)
            return std::nullopt;

        /* Fingerprint by the commit's root TREE, not the commit itself.

           The fingerprint keys the source-path-to-narHash cache
           (fetch-to-store.cc) that a lazy-trees mount consults before
           NAR-hashing the whole tree at mount time, and jj rewrites the
           working-copy commit freely: `jj describe`, `jj new` (an empty
           `@`), or any other metadata-only rewrite mints a new commit hash
           over a byte-identical tree. Keyed by commit, each such rewrite
           re-hashed the full tree (~10 s of every evaluation of a 173k-file
           repo, measured); keyed by tree it is a hit, which is sound
           because that cache stores a pure function of the tree's content.

           The flake eval cache also consumes this fingerprint, and itself
           appends `revCount` and `lastModified` (flake.cc) exactly so that
           content-based fingerprints stay sound for attributes the tree
           does not determine. What remains observable is `self.rev`: two
           commits over one tree, with equal revCount and the same
           one-second `lastModified` stamp, would share an eval-cache slot.
           jj stamps every rewrite with a fresh committer timestamp, so that
           takes two rewrites plus an evaluation within a single second.

           A commit carrying the legacy "jj:trees" header stores conflict
           variants outside its Git tree, so there the tree alone does not
           determine the materialised content; it keeps the commit
           fingerprint. Modern jj encodes conflicts in the tree itself. The
           "jj-tree:" prefix keeps these keys out of the plain-rev namespace
           other fetchers use. */
        try {
            auto repoPath = getActualPath(input);
            if (auto gitStore = gitStorePath(repoPath)) {
                auto repo = GitRepo::openRepo(*gitStore, {});
                if (repo->hasObject(*rev) && !repo->hasCommitExtraHeader(*rev, "jj:trees"))
                    return "jj-tree:" + repo->getTreeHash(*rev).gitRev();
            }
        } catch (Error & e) {
            debug("failed to resolve the tree of Jujutsu revision '%s': %s", rev->gitRev(), e.what());
        }

        return rev->gitRev();
    }
};

static auto rJjInputScheme = OnStartup([] { registerInputScheme(std::make_unique<JjInputScheme>()); });

} // namespace nix::fetchers
