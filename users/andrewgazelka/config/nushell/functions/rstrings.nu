use paths.nu *

def "path from string" [text: string] {
    if ($text | is-empty) {
        return null
    }

    let expanded = ($text | path expand)
    if ($expanded | path exists) and (($expanded | path type) == "file") {
        $expanded
    } else {
        null
    }
}

# Run strings over files referenced by its output, returning one structured row per string.
export def main [
    path?: path  # Initial file. If omitted, reads a path or list of paths from stdin.
    --max-depth (-d): int = 3  # Maximum reference depth. Initial files are depth zero.
    --threads (-t): int = 8  # Maximum files processed concurrently.
] {
    if $max_depth < 0 {
        error make { msg: "max depth must be nonnegative" }
    }
    if $threads < 1 {
        error make { msg: "threads must be positive" }
    }

    let input = $in
    let roots = (
        if $path == null { $input | path list } else { [ $path ] }
        | each { |source| $source | into string | path expand }
        | uniq
    )
    if ($roots | is-empty) {
        error make { msg: "rstrings requires at least one file path" }
    }

    let invalid = (
        $roots
        | where { |source|
            not ($source | path exists) or (($source | path type) != "file")
        }
    )
    if ($invalid | is-not-empty) {
        error make { msg: $"rstrings requires files: ($invalid | str join ', ')" }
    }

    mut frontier = $roots
    mut seen = $roots
    mut rows = []

    for depth in 0..$max_depth {
        if ($frontier | is-empty) {
            break
        }

        let strings_rows = (
            $frontier
            | par-each --keep-order --threads $threads { |source|
                do --capture-errors { ^strings -- $source }
                | lines
                | enumerate
                | each { |entry|
                    {
                        source: $source
                        depth: $depth
                        line: ($entry.index + 1)
                        text: $entry.item
                    }
                }
            }
            | flatten
        )
        let paths = (
            $strings_rows
            | select text
            | uniq
            | insert discovered_path { |row| path from string $row.text }
        )
        let depth_rows = ($strings_rows | join --left $paths text)
        $rows = $rows ++ $depth_rows

        let discovered = (
            $depth_rows
            | get discovered_path
            | compact
            | uniq
            | where { |source| $source not-in $seen }
        )
        $frontier = $discovered
        $seen = $seen ++ $discovered
    }

    $rows
}
