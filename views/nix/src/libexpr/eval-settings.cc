#include "nix/util/users.hh"
#include "nix/util/logging.hh"
#include "nix/store/globals.hh"
#include "nix/store/profiles.hh"
#include "nix/expr/eval.hh"
#include "nix/expr/eval-settings.hh"

namespace nix {

void DeprecatedWarnSetting::assign(const bool & v)
{
    value = v;
    warn("'%s' is deprecated, use '%s = %s' instead", name, targetName, v ? "warn" : "ignore");
    if (!target.overridden)
        target = v ? Diagnose::Warn : Diagnose::Ignore;
}

void DeprecatedWarnSetting::appendOrSet(bool newValue, bool append)
{
    assert(!append);
    assign(newValue);
}

void DeprecatedWarnSetting::override(const bool & v)
{
    overridden = true;
    assign(v);
}

/* Very hacky way to parse $NIX_PATH, which is colon-separated, but
   can contain URLs (e.g. "nixpkgs=https://bla...:foo=https://"). */
Strings EvalSettings::parseNixPath(const std::string & s)
{
    Strings res;

    auto p = s.begin();

    while (p != s.end()) {
        auto start = p;
        auto start2 = p;

        while (p != s.end() && *p != ':') {
            if (*p == '=')
                start2 = p + 1;
            ++p;
        }

        if (p == s.end()) {
            if (p != start)
                res.push_back(std::string(start, p));
            break;
        }

        if (*p == ':') {
            auto prefix = std::string(start2, s.end());
            if (EvalSettings::isPseudoUrl(prefix) || hasPrefix(prefix, "flake:")) {
                ++p;
                while (p != s.end() && *p != ':')
                    ++p;
            }
            res.push_back(std::string(start, p));
            if (p == s.end())
                break;
        }

        ++p;
    }

    return res;
}

EvalSettings::EvalSettings(bool & readOnlyMode, EvalSettings::LookupPathHooks lookupPathHooks)
    : readOnlyMode{readOnlyMode}
    , lookupPathHooks{lookupPathHooks}
{
    auto var = getEnv("NIX_ABORT_ON_WARN");
    if (var && (var == "1" || var == "yes" || var == "true"))
        builtinsAbortOnWarn = true;
}

Strings EvalSettings::getDefaultNixPath()
{
    Strings res;
    auto add = [&](const std::filesystem::path & p, const std::string & s = std::string()) {
        /* Use the non-throwing exists() overload: these default lookup-path
           entries are optional conveniences, so an inaccessible path must mean
           "absent", not an eval abort. The throwing overload turns an EPERM
           stat -- e.g. the macOS build sandbox denying access to the host's
           /nix/var/nix/profiles/per-user/root/channels while nix runs inside a
           derivation (recursive-nix, the C API unit tests) -- into a fatal
           filesystem_error for any --impure eval, even one that never uses the
           lookup path entry. */
        std::error_code ec;
        if (std::filesystem::exists(p, ec)) {
            if (s.empty()) {
                res.push_back(p.string());
            } else {
                res.push_back(s + "=" + p.string());
            }
        }
    };

    add(getNixDefExpr() / "channels");
    auto profilesDirOpts = settings.getProfileDirsOptions();
    add(rootChannelsDir(profilesDirOpts) / "nixpkgs", "nixpkgs");
    add(rootChannelsDir(profilesDirOpts));

    return res;
}

bool EvalSettings::isPseudoUrl(std::string_view s)
{
    if (s.compare(0, 8, "channel:") == 0)
        return true;
    size_t pos = s.find("://");
    if (pos == std::string::npos)
        return false;
    std::string scheme(s, 0, pos);
    return scheme == "http" || scheme == "https" || scheme == "file" || scheme == "channel" || scheme == "git"
           || scheme == "s3" || scheme == "ssh";
}

std::string EvalSettings::resolvePseudoUrl(std::string_view url)
{
    if (hasPrefix(url, "channel:"))
        return "https://channels.nixos.org/" + std::string(url.substr(8)) + "/nixexprs.tar.xz";
    else
        return std::string(url);
}

const std::string & EvalSettings::getCurrentSystem() const
{
    const auto & evalSystem = currentSystem.get();
    return evalSystem != "" ? evalSystem : settings.thisSystem.get();
}

std::filesystem::path getNixDefExpr()
{
    return settings.useXDGBaseDirectories ? getStateDir() / "defexpr" : getHome() / ".nix-defexpr";
}

} // namespace nix
