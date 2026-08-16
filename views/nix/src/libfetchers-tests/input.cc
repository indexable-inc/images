#include "nix/fetchers/fetch-settings.hh"
#include "nix/fetchers/attrs.hh"
#include "nix/fetchers/fetchers.hh"

#include <gtest/gtest.h>

#include <string>

namespace nix {

using fetchers::Attr;

struct InputFromAttrsTestCase
{
    fetchers::Attrs attrs;
    std::string expectedUrl;
    std::string description;
    fetchers::Attrs expectedAttrs = attrs;
};

class InputFromAttrsTest : public ::testing::WithParamInterface<InputFromAttrsTestCase>, public ::testing::Test
{
public:
    void SetUp() override
    {
        // The forge archive schemes (github/gitlab/sourcehut) are gated
        // on the flakes feature.
        experimentalFeatureSettings.experimentalFeatures.get().insert(Xp::Flakes);
    }
};

TEST_P(InputFromAttrsTest, attrsAreCorrectAndRoundTrips)
{
    fetchers::Settings fetchSettings;

    const auto & testCase = GetParam();

    auto input = fetchers::Input::fromAttrs(fetchSettings, fetchers::Attrs(testCase.attrs));

    EXPECT_EQ(input.toAttrs(), testCase.expectedAttrs);
    EXPECT_EQ(input.toURLString(), testCase.expectedUrl);

    auto input2 = fetchers::Input::fromAttrs(fetchSettings, input.toAttrs());
    EXPECT_EQ(input, input2);
    EXPECT_EQ(input.toAttrs(), input2.toAttrs());
}

INSTANTIATE_TEST_SUITE_P(
    InputFromAttrs,
    InputFromAttrsTest,
    ::testing::Values(
        // Test for issue #14429.
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"url", Attr("git+ssh://git@github.com/NixOS/nixpkgs")},
                    {"type", Attr("git")},
                },
            .expectedUrl = "git+ssh://git@github.com/NixOS/nixpkgs",
            .description = "strips_git_plus_prefix",
            .expectedAttrs =
                {
                    {"url", Attr("ssh://git@github.com/NixOS/nixpkgs")},
                    {"type", Attr("git")},
                },
        },
        // Forge archive inputs that request submodules or LFS construct
        // the equivalent git+https input, because forge tarballs cannot
        // contain that data (issues #13571, #14982).
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("github")},
                    {"owner", Attr("NixOS")},
                    {"repo", Attr("nix")},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
            .expectedUrl = "git+https://github.com/NixOS/nix.git?shallow=1&submodules=1",
            .description = "github_submodules_redirects_to_git",
            .expectedAttrs =
                {
                    {"type", Attr("git")},
                    {"url", Attr("https://github.com/NixOS/nix.git")},
                    {"shallow", Attr(Explicit<bool>{true})},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
        },
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("github")},
                    {"owner", Attr("NixOS")},
                    {"repo", Attr("nix")},
                    {"rev", Attr("0123456789abcdef0123456789abcdef01234567")},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
            .expectedUrl = "git+https://github.com/NixOS/nix.git?rev=0123456789abcdef0123456789abcdef01234567&"
                           "shallow=1&submodules=1",
            .description = "github_submodules_redirect_keeps_rev",
            .expectedAttrs =
                {
                    {"type", Attr("git")},
                    {"url", Attr("https://github.com/NixOS/nix.git")},
                    {"rev", Attr("0123456789abcdef0123456789abcdef01234567")},
                    {"shallow", Attr(Explicit<bool>{true})},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
        },
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("github")},
                    {"owner", Attr("NixOS")},
                    {"repo", Attr("nix")},
                    {"host", Attr("github.example.com")},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
            .expectedUrl = "git+https://github.example.com/NixOS/nix.git?shallow=1&submodules=1",
            .description = "github_submodules_redirect_keeps_host",
            .expectedAttrs =
                {
                    {"type", Attr("git")},
                    {"url", Attr("https://github.example.com/NixOS/nix.git")},
                    {"shallow", Attr(Explicit<bool>{true})},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
        },
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("github")},
                    {"owner", Attr("NixOS")},
                    {"repo", Attr("nix")},
                    {"lfs", Attr(Explicit<bool>{true})},
                },
            .expectedUrl = "git+https://github.com/NixOS/nix.git?lfs=1&shallow=1",
            .description = "github_lfs_redirects_to_git",
            .expectedAttrs =
                {
                    {"type", Attr("git")},
                    {"url", Attr("https://github.com/NixOS/nix.git")},
                    {"shallow", Attr(Explicit<bool>{true})},
                    {"lfs", Attr(Explicit<bool>{true})},
                },
        },
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("gitlab")},
                    {"owner", Attr("foo")},
                    {"repo", Attr("bar")},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
            .expectedUrl = "git+https://gitlab.com/foo/bar.git?shallow=1&submodules=1",
            .description = "gitlab_submodules_redirects_to_git",
            .expectedAttrs =
                {
                    {"type", Attr("git")},
                    {"url", Attr("https://gitlab.com/foo/bar.git")},
                    {"shallow", Attr(Explicit<bool>{true})},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
        },
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("sourcehut")},
                    {"owner", Attr("~foo")},
                    {"repo", Attr("bar")},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
            .expectedUrl = "git+https://git.sr.ht/~foo/bar?shallow=1&submodules=1",
            .description = "sourcehut_submodules_redirects_to_git",
            .expectedAttrs =
                {
                    {"type", Attr("git")},
                    {"url", Attr("https://git.sr.ht/~foo/bar")},
                    {"shallow", Attr(Explicit<bool>{true})},
                    {"submodules", Attr(Explicit<bool>{true})},
                },
        },
        // Plain forge inputs keep archive semantics (and hashes).
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("github")},
                    {"owner", Attr("NixOS")},
                    {"repo", Attr("nix")},
                },
            .expectedUrl = "github:NixOS/nix",
            .description = "github_without_submodules_stays_github",
        },
        // An explicit false is the archive default and is normalized
        // away, so locked forge inputs never carry the attribute.
        InputFromAttrsTestCase{
            .attrs =
                {
                    {"type", Attr("github")},
                    {"owner", Attr("NixOS")},
                    {"repo", Attr("nix")},
                    {"submodules", Attr(Explicit<bool>{false})},
                },
            .expectedUrl = "github:NixOS/nix",
            .description = "github_submodules_false_is_dropped",
            .expectedAttrs =
                {
                    {"type", Attr("github")},
                    {"owner", Attr("NixOS")},
                    {"repo", Attr("nix")},
                },
        }),
    [](const ::testing::TestParamInfo<InputFromAttrsTestCase> & info) { return info.param.description; });

class InputFromURLTest : public ::testing::Test
{
public:
    void SetUp() override
    {
        experimentalFeatureSettings.experimentalFeatures.get().insert(Xp::Flakes);
    }
};

/* Previously `github:...?submodules=1` silently dropped the query
   parameter and fetched a tarball with empty submodule directories
   (issue #14982). */
TEST_F(InputFromURLTest, githubSubmodulesUrlRedirectsToGit)
{
    fetchers::Settings fetchSettings;
    auto input = fetchers::Input::fromURL(fetchSettings, "github:NixOS/nix?submodules=1");
    EXPECT_EQ(fetchers::maybeGetStrAttr(input.toAttrs(), "type"), std::optional<std::string>("git"));
    EXPECT_EQ(input.toURLString(), "git+https://github.com/NixOS/nix.git?shallow=1&submodules=1");
}

TEST_F(InputFromURLTest, githubSubmodulesFalseUrlStaysGithub)
{
    fetchers::Settings fetchSettings;
    auto input = fetchers::Input::fromURL(fetchSettings, "github:NixOS/nix?submodules=0");
    EXPECT_EQ(fetchers::maybeGetStrAttr(input.toAttrs(), "type"), std::optional<std::string>("github"));
    EXPECT_EQ(input.toURLString(), "github:NixOS/nix");
}

} // namespace nix
