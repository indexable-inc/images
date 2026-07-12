def "nu-complete tmux sessions" [] {
    let result = (
        do {
            ^tmux list-sessions -F "#{session_name}\t#{session_windows} windows#{?session_attached, attached,}"
        } | complete
    )

    if $result.exit_code != 0 {
        return []
    }

    $result.stdout
    | lines
    | each { |line|
        let parts = ($line | split row "\t")
        {
            value: ($parts | get 0)
            description: (if (($parts | length) > 1) { $parts | get 1 } else { "" })
        }
    }
}

export extern "tmux attach" [
    session?: string@"nu-complete tmux sessions"
    -t: string@"nu-complete tmux sessions"
    -d
    -r
    -x
]

export extern "tmux attach-session" [
    session?: string@"nu-complete tmux sessions"
    -t: string@"nu-complete tmux sessions"
    -d
    -r
    -x
]

export extern "tmux kill-session" [
    -t: string@"nu-complete tmux sessions"
    -a
    -C
]
