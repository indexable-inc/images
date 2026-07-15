use std/assert
use ../functions/rstrings.nu *

def fixture [] {
    let directory = (^mktemp -d | str trim | path expand)
    let root = ($directory | path join "root.bin")
    let child = ($directory | path join "child.bin")

    $"root text\n($child)\n($child)\n" | save --raw $root
    $"child text\n($root)\n" | save --raw $child

    { directory: $directory, root: $root, child: $child }
}

#[test]
def test_depth_and_cycle_deduplication [] {
    let files = (fixture)
    let root_only = (rstrings $files.root --max-depth 0 --threads 2)
    let recursive = (rstrings $files.root --max-depth 2 --threads 2)

    assert equal ($root_only | get depth | uniq) [0]
    assert equal ($recursive | where depth == 1 | get source | uniq) [$files.child]
    assert equal ($recursive | where depth == 2 | length) 0

    rm --recursive --force $files.directory
}

#[test]
def test_pipeline_and_input_validation [] {
    let files = (fixture)
    let rows = ([$files.root $files.child] | rstrings --max-depth 0)

    assert equal ($rows | get source | uniq | sort) ([$files.root $files.child] | sort)
    assert error { rstrings }
    assert error { rstrings $files.directory }
    assert error { rstrings $files.root --max-depth -1 }
    assert error { rstrings $files.root --threads 0 }

    rm --recursive --force $files.directory
}

export def run-all [] {
    test_depth_and_cycle_deduplication
    test_pipeline_and_input_validation
}
