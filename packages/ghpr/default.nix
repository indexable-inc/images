# ghpr: turn the unpushed commits on the current branch into a pull request
# in one shot. Ported from github:harivansh-afk/nix (scripts/bin/ghpr.sh)
# as a general tool: nothing in it is user-specific.
#
# Flow: with commits ahead of origin/<upstream> (default `main`; staged-only
# changes get a commit dialog first), it derives a branch name from the first
# unpushed commit subject, moves the commits onto that branch, resets the
# starting branch back to origin/<upstream>, pushes, and opens a PR with
# `gh pr create --fill` (browser when available, terminal otherwise),
# printing the PR URL.
#
# Kept as checked bash (ix.writeBashApplication: `bash -n` + shellcheck in
# the build) rather than rewritten in Nushell: it is a byte-faithful port of
# a tool with an established behaviour, all git porcelain glue.
{
  ix,
  pkgs,
}:
ix.writeBashApplication pkgs {
  name = "ghpr";
  meta = {
    description = "Turn unpushed commits into a branch + GitHub PR in one shot";
    mainProgram = "ghpr";
  };
  runtimeInputs = [
    pkgs.coreutils
    pkgs.gh
    pkgs.git
    pkgs.gnugrep
    pkgs.gnused
  ];
  text = ''
    base=$(git rev-parse --abbrev-ref HEAD)
    upstream="''${1:-main}"
    remote_ref="origin/$upstream"
    unpushed=$(git log "$remote_ref"..HEAD --oneline 2>/dev/null)

    if [[ -z "$unpushed" ]]; then
      if git diff --cached --quiet; then
        echo "No unpushed commits and no staged changes"
        exit 1
      fi

      echo "No unpushed commits, but staged changes found. Opening commit dialog..."
      git commit
    fi

    msg=$(git log "$remote_ref"..HEAD --format='%s' --reverse | head -1)
    branch=$(echo "$msg" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g' | sed 's/--*/-/g' | sed 's/^-//;s/-$//')

    git checkout -b "$branch"
    git checkout "$base"
    git reset --hard "$remote_ref"
    git checkout "$branch"

    git push -u origin "$branch"
    gh pr create --base "$upstream" --fill --web 2>/dev/null || gh pr create --base "$upstream" --fill
    gh pr view "$branch" --json url -q '.url'
  '';
}
