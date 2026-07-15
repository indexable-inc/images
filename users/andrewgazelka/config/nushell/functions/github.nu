# Parse conventional commit and return structured data
def parse-conventional-commit [commit: string] {
    # Try to match type(scope): message or type: message format
    let pattern = '^(?<type>[a-z]+)(?:\((?<scope>[^\)]+)\))?(?<breaking>!)?: (?<message>.+)$'
    let parsed = ($commit | parse --regex $pattern)

    if ($parsed | is-empty) {
        # Fallback: try to extract scope from parentheses anywhere in message
        let scope_pattern = '(?<prefix>.*?)\((?<scope>[^\)]+)\)(?<suffix>.*)'
        let scope_match = ($commit | parse --regex $scope_pattern)

        if ($scope_match | is-empty) {
            return {
                type: "other"
                scope: null
                breaking: false
                message: $commit
            }
        } else {
            let m = ($scope_match | first)
            return {
                type: "other"
                scope: $m.scope
                breaking: false
                message: ($m.prefix + $m.suffix | str trim)
            }
        }
    }

    let match = ($parsed | first)
    {
        type: $match.type
        scope: ($match.scope? | default null)
        breaking: ($match.breaking? == "!")
        message: $match.message
    }
}

# Get color for commit type
def get-commit-style [type: string] {
    match $type {
        "feat" => { label: "Feature", color: "green" }
        "fix" => { label: "Fix", color: "red" }
        "docs" => { label: "Docs", color: "blue" }
        "style" => { label: "Style", color: "magenta" }
        "refactor" => { label: "Refactor", color: "yellow" }
        "perf" => { label: "Perf", color: "yellow" }
        "test" => { label: "Test", color: "cyan" }
        "build" => { label: "Build", color: "cyan" }
        "ci" => { label: "CI", color: "cyan" }
        "chore" => { label: "Chore", color: "dark_gray" }
        _ => { label: "Other", color: "white" }
    }
}

# Display git log with styled conventional commits
export def glc [
    --limit: int = 10  # Number of commits to show
] {
    git log --pretty=format:'%h|%s' -n $limit
    | lines
    | each {|line|
        let parts = ($line | split row '|')
        let hash = ($parts | first)
        let message = ($parts | last)
        let parsed = (parse-conventional-commit $message)
        let style = (get-commit-style $parsed.type)

        {
            type: $style.label
            scope: ($parsed.scope | default "")
            message: $parsed.message
        }
    }
}

# Get the main branch name (fast, uses local cache)
export def git_main_branch [] {
    # origin/HEAD is a symbolic ref created during `git clone` that points to
    # the remote's default branch. It's stored locally in .git/refs/remotes/origin/HEAD
    # as something like "ref: refs/remotes/origin/main"
    #
    # git rev-parse --abbrev-ref origin/HEAD resolves this to "origin/main"
    # This is instant since it reads local files, no network call needed.
    #
    # If origin/HEAD doesn't exist (e.g., repo was created locally then pushed),
    # you can set it manually with: git remote set-head origin --auto
    let local = (do { git rev-parse --abbrev-ref origin/HEAD } | complete)
    if $local.exit_code == 0 {
        $local.stdout | str trim | str replace "origin/" ""
    } else {
        # Fallback: query GitHub API (slower, requires network)
        gh api repos/{owner}/{repo} --jq '.default_branch' | str trim
    }
}


export def watch-gh-runs [
    --commit: string = ""  # Specific commit to watch (default: current HEAD)
    --interval: int = 10   # Polling interval in seconds
] {
    let target_commit = if ($commit | is-empty) {
        (git rev-parse HEAD | str trim)
    } else {
        $commit
    }

    loop {
        let runs = (gh run list --commit $target_commit --json databaseId,status,conclusion,name | from json)

        if ($runs | length) > 0 {
            let has_failure = ($runs | any {|r| $r.conclusion == "failure"})
            let all_success = ($runs | all {|r| $r.status == "completed" and $r.conclusion == "success"})

            if $has_failure or $all_success {
                return $runs
            }
        }

        sleep ($interval | into duration --unit sec)
    }
}

const GH_PR_PROMPT_CACHE_DIR = "~/.cache/nushell/github-pr-prompt"

def github-normalize-repo [remote: string] {
    let normalized = ($remote | str trim | str replace --regex '\.git$' '')
    let parsed = (
        $normalized
        | parse --regex '^(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)(?<owner>[^/]+)/(?<repo>[^/]+)$'
    )

    if ($parsed | is-empty) {
        ""
    } else {
        let repo = ($parsed | first)
        $"($repo.owner)/($repo.repo)"
    }
}

def github-pr-prompt-cache-file [repo: string] {
    let cache_dir = ($GH_PR_PROMPT_CACHE_DIR | path expand)
    let file_name = ($repo | str replace --all "/" "__")
    $cache_dir | path join $"($file_name).txt"
}

def github-pr-prompt-lock-file [repo: string] {
    let cache_dir = ($GH_PR_PROMPT_CACHE_DIR | path expand)
    let file_name = ($repo | str replace --all "/" "__")
    $cache_dir | path join $"($file_name).lock"
}

def github-path-is-fresh [path: path, max_age: duration] {
    if not ($path | path exists) {
        return false
    }

    let modified = (ls $path | get 0.modified)
    ((date now) - $modified) < $max_age
}

def github-pr-check-token [check: record] {
    let conclusion = ($check.conclusion? | default "" | str upcase)
    if not ($conclusion | is-empty) {
        return $conclusion
    }

    let status = ($check.status? | default "" | str upcase)
    if not ($status | is-empty) {
        return $status
    }

    $check.state? | default "" | str upcase
}

def github-pr-check-state [rollup: list] {
    let checks = (
        $rollup
        | each {|check| github-pr-check-token $check }
        | where {|state| not ($state | is-empty) and ($state not-in [SKIPPED NEUTRAL]) }
    )

    if ($checks | is-empty) {
        return "wip"
    }

    let bad_states = [FAILURE ERROR CANCELLED TIMED_OUT ACTION_REQUIRED STARTUP_FAILURE]
    let good_states = [SUCCESS COMPLETED]

    if ($checks | any {|state| $state in $bad_states }) {
        "bad"
    } else if ($checks | all {|state| $state in $good_states }) {
        "good"
    } else {
        "wip"
    }
}

def github-pr-prompt-color [state: string] {
    match $state {
        "good" => (ansi green_bold)
        "bad" => (ansi red_bold)
        _ => (ansi yellow_bold)
    }
}

export def github-current-repo [] {
    let inside_repo = (do { git rev-parse --is-inside-work-tree } | complete)
    if $inside_repo.exit_code != 0 {
        return ""
    }

    let remote = (do { git config --get remote.origin.url } | complete)
    if $remote.exit_code != 0 {
        return ""
    }

    github-normalize-repo $remote.stdout
}

export def github-pr-prompt-refresh [
    --repo: string = ""
] {
    let selected_repo = if ($repo | is-empty) { github-current-repo } else { $repo }
    if ($selected_repo | is-empty) {
        return
    }

    let cache_dir = ($GH_PR_PROMPT_CACHE_DIR | path expand)
    mkdir $cache_dir

    let cache_file = (github-pr-prompt-cache-file $selected_repo)
    let lock_file = (github-pr-prompt-lock-file $selected_repo)
    "refreshing" | save --force $lock_file

    let result = (
        do {
            gh pr list --repo $selected_repo --author @me --state open --limit 20 --json headRefName,isDraft,statusCheckRollup,url
        } | complete
    )

    if $result.exit_code == 0 {
        let prs = ($result.stdout | from json)
        let output = (
            $prs
            | each {|pr|
                let state = if $pr.isDraft { "wip" } else { github-pr-check-state $pr.statusCheckRollup }
                {
                    branch: $pr.headRefName
                    url: $pr.url
                    state: $state
                    rank: (match $state {
                        "good" => 0
                        "wip" => 1
                        _ => 2
                    })
                }
            }
            | sort-by rank branch
            | each {|pr|
                let label = $"(github-pr-prompt-color $pr.state)($pr.branch)(ansi reset)"
                $pr.url | ansi link --text $label
            }
            | str join " "
        )
        $output | save --force $cache_file
    }

    rm --force $lock_file
}

export def github-pr-prompt-refresh-current-if-stale [
    --max-age: duration = 1min
    --lock-max-age: duration = 2min
] {
    let repo = (github-current-repo)
    if ($repo | is-empty) {
        return
    }

    let cache_file = (github-pr-prompt-cache-file $repo)
    let lock_file = (github-pr-prompt-lock-file $repo)
    if (github-path-is-fresh $cache_file $max_age) or (github-path-is-fresh $lock_file $lock_max_age) {
        return
    }

    job spawn --description $"refresh GitHub PR prompt ($repo)" { github-pr-prompt-refresh --repo $repo } | ignore
}

export def github-pr-prompt [] {
    let repo = (github-current-repo)
    if ($repo | is-empty) {
        return
    }

    let cache_file = (github-pr-prompt-cache-file $repo)
    if not ($cache_file | path exists) {
        return
    }

    let output = (open --raw $cache_file | str trim)
    if not ($output | is-empty) {
        $output
    }
}
