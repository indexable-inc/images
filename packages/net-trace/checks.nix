# net-trace comment-path acceptance (#4031), imported by lib/per-system.nix
# into the per-system check catalog. The test body lives here as an mkCheck
# recipe rather than a committed script because new standalone shell is
# fenced (#3823); recipes stay the one sanctioned shell surface.
{
  lib,
  pkgs,
  paths,
  mkCheck,
}: let
  fs = lib.fileset;
  # Exactly what the check reads: the fixtures and the workflow whose
  # embedded jq it pins, so it reruns only when either changes. The source
  # root stays the repo root so the workflow keeps its repo-relative path.
  testSource = fs.toSource {
    inherit (paths) root;
    fileset = fs.intersection (fs.gitTracked paths.root) (
      fs.unions [
        ./tests
        (paths.root + "/.github/workflows/check.yml")
      ]
    );
  };
in {
  # Exercises the trusted half of the net-trace PR comment: the validate and
  # render jq embedded in check.yml, extracted from the YAML so this check
  # cannot drift from what the trusted comment job runs. The good summary
  # must pass validation and render the golden comment byte for byte; each
  # hostile fixture pins one fail-closed rule (host charset, label shape,
  # scheme enumeration, required phases array). The proxy and summary logic
  # live in the `net-trace` Rust crate with their own tests.
  net-trace-test = mkCheck "net-trace-test" {
    nativeBuildInputs = [
      pkgs.bash
      pkgs.coreutils
      pkgs.diffutils
      pkgs.jq
      pkgs.yq-go
    ];
    script = ''
      fixtures=${testSource}/packages/net-trace/tests/fixtures
      workflow=${testSource}/.github/workflows/check.yml
      yq '.jobs.net-trace-comment.steps[] | select(.name == "Validate summary schema").run' "$workflow" > validate.sh
      yq '.jobs.net-trace-comment.steps[] | select(.name == "Render comment").run' "$workflow" > render.sh

      cp "$fixtures/good.json" net-trace-summary.json
      bash validate.sh
      for bad in bad-host bad-label bad-scheme missing; do
        cp "$fixtures/$bad.json" net-trace-summary.json
        if bash validate.sh 2>/dev/null; then
          echo "validate $bad: accepted a hostile summary" >&2
          exit 1
        fi
      done

      cp "$fixtures/good.json" net-trace-summary.json
      bash render.sh
      diff -u "$fixtures/golden.md" comment.md
    '';
  };
}
