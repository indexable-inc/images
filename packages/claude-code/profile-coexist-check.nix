# Two wrappers from this package have to coexist in ONE user profile
# (ENG-12737): the ordinary `claude` and an overridden strict one, typically
# `claude-code.override { kernelOnly = true; binName = "claude-kernel"; }`
# installed side by side from a Home Manager `home.packages`.
#
# `pkgs.buildEnv` refuses any relative path that two of its inputs both
# provide, so every path this package installs outside `bin/${binName}` has to
# be derived from `binName` too. That was not true when the scoped wrapper
# first shipped: `libexec/Claude Code` and `share/claude-code-launch-spec.json`
# were fixed names, and the profile would not build.
#
# The bug is only visible when the two derivations are actually merged. An eval
# assertion that both packages are in `home.packages` passes happily while the
# profile is unbuildable -- that is exactly the check that existed at the time
# and exactly what it missed -- so this one does the merge.
{
  pkgs,
  claudeCode,
}: let
  strict = claudeCode.override {
    kernelOnly = true;
    binName = "claude-kernel";
  };
  # The real shape from the field: both wrappers, one profile.
  profile = pkgs.buildEnv {
    name = "claude-code-profile-coexist";
    paths = [
      claudeCode
      strict
    ];
  };
in
  pkgs.runCommand "claude-code-profile-coexist-check" {
    __structuredAttrs = true;
    inherit profile;
  } ''
    # Reaching here at all means buildEnv merged the two without a conflicting
    # subpath, which is the property under test. The assertions below keep a
    # future "fix" from buying that by dropping a wrapper on the floor.
    for bin in claude claude-kernel; do
      if [ ! -e "$profile/bin/$bin" ]; then
        echo "claude-code-profile-coexist: $profile/bin/$bin missing from the merged profile." >&2
        echo "Both wrappers must survive the merge; a profile with only one of them" >&2
        echo "is not the thing this check exists to prove." >&2
        exit 1
      fi
    done

    # Each wrapper must still reach its OWN launch spec and helper. Sharing one
    # would merge cleanly and then run the wrong configuration, which is a
    # worse failure than the build error this check replaced.
    # `-L`: buildEnv symlinks whole directories when only one input provides
    # them, so `share/claude` is a link into that wrapper's store path and a
    # find that does not follow links descends into neither and counts zero.
    # (Written without it first, which is how this comment got here.)
    specs=$(find -L "$profile/share" -name 'claude-code-launch-spec.json' | wc -l | tr -d ' ')
    helpers=$(find -L "$profile/libexec" -name 'Claude Code' | wc -l | tr -d ' ')
    if [ "$specs" != "2" ] || [ "$helpers" != "2" ]; then
      echo "claude-code-profile-coexist: expected 2 launch specs and 2 helpers in the" >&2
      echo "merged profile, found $specs and $helpers. Each wrapper needs its own, or" >&2
      echo "one of them is running the other's configuration." >&2
      find -L "$profile/share" "$profile/libexec" >&2
      exit 1
    fi

    mkdir -p "$out"
  ''
