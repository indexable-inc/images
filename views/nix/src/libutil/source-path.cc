#include "nix/util/source-path.hh"
#include "nix/util/source-read-hook.hh"
#include "nix/util/hash.hh"
#include "nix/util/serialise.hh"

namespace nix {

std::string_view SourcePath::baseName() const
{
    return path.baseName().value_or("source");
}

SourcePath SourcePath::parent() const
{
    auto p = path.parent();
    assert(p);
    return {accessor, std::move(*p)};
}

/** How a stat answer is written down, so that two runs can compare them. */
static std::string showStatObservation(const std::optional<SourceAccessor::Stat> & st)
{
    if (!st)
        return "absent";
    auto s = SourceAccessor::Stat(*st).typeString();
    if (st->type == SourceAccessor::tRegular && st->isExecutable)
        s += ",executable";
    return s;
}

std::string SourcePath::readFile() const
{
    recordSourceRead(*accessor, path, SourceReadKind::contents);
    auto contents = accessor->readFile(path);
    recordSourceObserved(*accessor, path, SourceReadKind::contents, contents);
    return contents;
}

bool SourcePath::pathExists() const
{
    recordSourceRead(*accessor, path, SourceReadKind::metadata);
    /* Through maybeLstat rather than pathExists, so that the recorded
       observation is the type and not just the boolean this caller wanted. A
       path that changes from a file to a directory is a changed input even
       though both answers are "it exists". */
    if (wantSourceObserved()) [[unlikely]] {
        auto st = accessor->maybeLstat(path);
        recordSourceObserved(*accessor, path, SourceReadKind::metadata, showStatObservation(st));
        return st.has_value();
    }
    return accessor->pathExists(path);
}

SourceAccessor::Stat SourcePath::lstat() const
{
    recordSourceRead(*accessor, path, SourceReadKind::metadata);
    auto st = accessor->lstat(path);
    recordSourceObserved(*accessor, path, SourceReadKind::metadata, showStatObservation(st));
    return st;
}

std::optional<SourceAccessor::Stat> SourcePath::maybeLstat() const
{
    recordSourceRead(*accessor, path, SourceReadKind::metadata);
    auto st = accessor->maybeLstat(path);
    recordSourceObserved(*accessor, path, SourceReadKind::metadata, showStatObservation(st));
    return st;
}

SourceAccessor::DirEntries SourcePath::readDirectory() const
{
    recordSourceRead(*accessor, path, SourceReadKind::listing);
    auto entries = accessor->readDirectory(path);
    if (wantSourceObserved()) [[unlikely]] {
        /* The names and their types, in the map's order, which is sorted. A
           file added to this directory changes this without changing a byte of
           any file, which is the case a contents-only read set misses. */
        std::string observed;
        for (auto & [name, type] : entries) {
            observed += name;
            observed += ':';
            observed += type ? SourceAccessor::Stat{.type = *type}.typeString() : "unknown";
            observed += '\n';
        }
        recordSourceObserved(*accessor, path, SourceReadKind::listing, observed);
    }
    return entries;
}

std::string SourcePath::readLink() const
{
    recordSourceRead(*accessor, path, SourceReadKind::link);
    auto target = accessor->readLink(path);
    recordSourceObserved(*accessor, path, SourceReadKind::link, target);
    return target;
}

void SourcePath::dumpPath(Sink & sink, PathFilter & filter) const
{
    recordSourceRead(*accessor, path, SourceReadKind::subtree);
    if (!wantSourceObserved())
        return accessor->dumpPath(path, sink, filter);
    /* Hash what is dumped rather than recording the path alone. A derivation
       whose source is a whole tree reads that tree through here, so without
       this its input carries no content identity and a changed tree reads as
       unchanged, which is unsound in the direction that serves a stale
       result. */
    HashSink hashSink{HashAlgorithm::SHA256};
    TeeSink tee{sink, hashSink};
    accessor->dumpPath(path, tee, filter);
    recordSourceObserved(
        *accessor, path, SourceReadKind::subtree, hashSink.finish().hash.to_string(HashFormat::Base16, false));
}

std::optional<std::filesystem::path> SourcePath::getPhysicalPath() const
{
    return accessor->getPhysicalPath(path);
}

std::string SourcePath::to_string() const
{
    return accessor->showPath(path);
}

SourcePath SourcePath::operator/(const CanonPath & x) const
{
    return {accessor, path / x};
}

SourcePath SourcePath::operator/(std::string_view c) const
{
    return {accessor, path / c};
}

bool SourcePath::operator==(const SourcePath & x) const noexcept
{
    return std::tie(*accessor, path) == std::tie(*x.accessor, x.path);
}

std::strong_ordering SourcePath::operator<=>(const SourcePath & x) const noexcept
{
    return std::tie(*accessor, path) <=> std::tie(*x.accessor, x.path);
}

std::ostream & operator<<(std::ostream & str, const SourcePath & path)
{
    str << path.to_string();
    return str;
}

} // namespace nix
