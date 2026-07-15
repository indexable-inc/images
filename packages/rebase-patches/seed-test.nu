use dag-lib.nu *

def main [source: string, expected_tree: string] {
  let scratch = (mktemp --directory --tmpdir "rebase-patches-seed-test.XXXXXX")
  let base = (dag seed-base-repo $source $scratch)
  let actual_tree = (git -C $scratch rev-parse $"($base)^{tree}" | str trim)

  if $actual_tree != $expected_tree {
    error make {
      msg: $"seeded tree ($actual_tree) differs from source tree ($expected_tree)"
    }
  }
}
