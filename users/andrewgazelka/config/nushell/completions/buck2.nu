def "nu-complete buck2 targets" [] {
    let result = (do { ^buck2 targets "//..." } | complete)
    if $result.exit_code != 0 { return [] }
    $result.stdout | lines | where $it != '' | each { str replace 'root//' '//' }
}

export extern "buck2 build" [
    --show-output
    --show-full-output
    --show-json-output
    --no-remote-cache
    --local-only
    --prefer-local
    --prefer-remote
    --keep-going
    --num-threads: int
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    ...targets: string@"nu-complete buck2 targets"
]

export extern "buck2 run" [
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    target: string@"nu-complete buck2 targets"
    ...args: string
]

export extern "buck2 test" [
    --keep-going
    --exclude: string
    --include: string
    --local-only
    --num-threads: int
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    ...targets: string@"nu-complete buck2 targets"
]

export extern "buck2 targets" [
    --json
    --stats
    --streaming
    --keep-going
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    ...patterns: string@"nu-complete buck2 targets"
]

export extern "buck2 query" [
    --output-attribute: string
    --json
    --dot
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    ...query: string
]

export extern "buck2 uquery" [
    --output-attribute: string
    --json
    --dot
    --config(-c): string
    --verbose(-v)
    ...query: string
]

export extern "buck2 cquery" [
    --output-attribute: string
    --json
    --dot
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    ...query: string
]

export extern "buck2 aquery" [
    --output-attribute: string
    --json
    --dot
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    ...query: string
]

export extern "buck2 clean" [
    --verbose(-v)
]

export extern "buck2 install" [
    --config(-c): string
    --target-platforms: string
    --modifier(-m): string
    --verbose(-v)
    target: string@"nu-complete buck2 targets"
    ...args: string
]
