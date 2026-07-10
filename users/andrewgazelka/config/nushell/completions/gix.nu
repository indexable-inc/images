def "nu-complete gix clone repos" [context: string, position: int] {
    let url = ($context | str trim | split row " " | last)

    if not ($url | str starts-with "https://github.com/") {
        return []
    }

    let path = $url | str replace "https://github.com/" ""
    let parts = $path | split row "/"

    if ($parts | length) < 1 or ($parts | first) == "" {
        return []
    }

    let owner = $parts | first

    let results = (do { ^gh repo list $owner --limit 100 --json name } | complete)
    if $results.exit_code != 0 { return [] }

    $results.stdout
    | from json
    | get name
    | each { |name| $"https://github.com/($owner)/($name)" }
}

export extern "gix clone" [
    url?: string@"nu-complete gix clone repos"
    --bare
    --depth: int
    ...rest: string
]
