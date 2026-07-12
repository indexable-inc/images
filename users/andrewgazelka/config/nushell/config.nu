$env.config.show_banner = false

# Prompt indicators
$env.PROMPT_INDICATOR = ""
$env.PROMPT_INDICATOR_VI_INSERT = ""
$env.PROMPT_INDICATOR_VI_NORMAL = ""


def "config claude" [
      --default (-d)  # Print default config
  ] {
      if $default {
          print "# Default Claude config..."
      } else {
          ^$env.EDITOR ~/.claude/CLAUDE.md
      }
  }

# Default system prompt override is empty (built-in prompt stays in effect).
# Set `$env.CLAUDE_SYS_PROMPT = "..."` (or export it from env.nu) to override.
$env.CLAUDE_SYS_PROMPT = ($env.CLAUDE_SYS_PROMPT? | default "The secret word is tofu.")

# Wrap `claude` so a non-empty CLAUDE_SYS_PROMPT is injected as --system-prompt
# (full override, not append). --dangerously-skip-permissions still comes from
# the `cl`/`cls`/`clh`/... abbreviations.
# def --wrapped claude [...args] {
#     if ($env.CLAUDE_SYS_PROMPT | is-empty) {
#         ^claude ...$args
#     } else {
#         ^claude --system-prompt $env.CLAUDE_SYS_PROMPT ...$args
#     }
# }

# Start Claude Code in debug mode by default. Shadows the external `claude` so
# the bare command and every `cl*` alias (which expand to `claude ...`) inherit
# `--debug`; `^claude` calls the real binary to avoid recursion. Only affects
# interactive nushell, so `claude -p` in scripts and the zsh RemoteCommand path
# are untouched. `--debug` is idempotent, so the now-redundant `cld` alias is fine.
# def --wrapped claude [...args] {
#     ^claude --debug ...$args
# }

# use std-rfc/iter recurse  # TODO: install std-rfc module
use std iter
# use std-rfc/kv *  # TODO: install std-rfc module

# Load custom functions
use ($nu.default-config-dir | path join "functions") *

def nu-commands [] {
    crdt | get chats.messages | each { where name? == "nushell" | select input?.command? output?.raw_sent_to_model? startTime? endTime? | rename input output start end | compact start end | into datetime start end | update start { $in | date to-timezone local } | update end { $in | date to-timezone local } } | sort-by start
}

def should-explore-output [threshold: int = 6, bytes: int = 2000] {
    let data = $in
    let kind = ($data | describe)
    let first_kind = try { $data | first | describe } catch { "" }
    let rows = try { $data | length } catch { 0 }
    let fields = try { $data | columns | length } catch { 0 }
    let size = try { $data | to json -r | str length } catch { 0 }
    let scalar_list = ($kind =~ '^list') and (not ($first_kind =~ '^record'))

    ($size > $bytes) or ((not $scalar_list) and (($rows > $threshold) or ($fields > $threshold)))
}

def maybe-unix-time [field: string] {
    let value = $in
    if ($value | describe) == "nothing" {
        return $value
    }

    let lower = ($field | str downcase)
    let text = ($value | into string)

    if (not ($lower | str contains "time")) or (not ($text =~ '^-?\d+$')) {
        return $value
    }

    let digits = ($text | str replace -r '^-?' '' | str length)

    if $digits >= 18 {
        try { $value | into datetime } catch { $value }
    } else if $digits >= 13 {
        try { ($value * 1000000) | into datetime } catch { $value }
    } else if $digits >= 10 {
        try { $value | into datetime -f '%s' } catch { $value }
    } else {
        $value
    }
}

def normalize-explore-output [field: string = ""] {
    let value = $in
    let kind = ($value | describe)

    if ($kind =~ '^record') {
        $value
        | items {|key, val| { key: $key, value: ($val | normalize-explore-output $key) } }
        | reduce -f {} {|row, acc| $acc | insert $row.key $row.value }
    } else if ($kind =~ '^(list|table)') {
        $value | each {|item| $item | normalize-explore-output $field }
    } else {
        $value | maybe-unix-time $field
    }
}

def copy-explore-peek [] {
    let value = $in
    let text = if ($value | describe) == "string" { $value } else { $value | to nuon }

    if (which "clip copy" | is-not-empty) {
        $text | clip copy
    } else if (which pbcopy | is-not-empty) {
        $text | ^pbcopy
    }

    $value
}

def render-explore-output [] {
    let data = ($in | normalize-explore-output)
    let columns = (try { term size | get columns } catch { 0 })

    if $nu.is-interactive and ($data | should-explore-output) {
        $data | explore -ip | copy-explore-peek
    } else if $columns >= 100 {
        $data | table -e
    } else {
        $data | table
    }
}

def locks [] {
    let rows = (
        nix flake metadata --json
        | from json
        | get locks.nodes
        | transpose node data
        | each {|row|
            let locked = ($row.data.locked? | default {})
            {
                node: $row.node
                lastModified: (if (($locked.lastModified? | default null) == null) { null } else { $locked.lastModified | into datetime -f '%s' })
                commitSha: ($locked.rev? | default null)
            }
        }
    )

    ($rows | where lastModified == null) ++ ($rows | where lastModified != null | sort-by lastModified)
}

def gens [] {
    let state_home = ($env.XDG_STATE_HOME? | default ($env.HOME | path join ".local/state"))
    let user_profiles = ($state_home | path join "nix/profiles")
    let profile_dir = if (($user_profiles | path join "home-manager") | path exists) {
        $user_profiles
    } else {
        $"/nix/var/nix/profiles/per-user/($env.USER)"
    }
    let links = (glob ($profile_dir | path join "home-manager-*-link"))

    if ($links | is-empty) {
        return []
    }

    let current = (^readlink ($profile_dir | path join "home-manager"))

    ^ls --color=never -gG --time-style=long-iso --sort=time ...$links
    | lines
    | parse --regex '^\S+\s+\d+\s+\d+\s+(?<date>\d{4}-\d{2}-\d{2})\s+(?<time>\d{2}:\d{2})\s+(?<link>\S+)\s+->\s+(?<store_path>/nix/store/\S+)$'
    | insert datetime {|row| $"($row.date) ($row.time)" | into datetime -f "%Y-%m-%d %H:%M" }
    | update link { path basename }
    | insert id {|row| $row.link | parse "home-manager-{id}-link" | get 0.id | into int }
    | insert current {|row| $row.link == $current }
    | select id datetime current store_path link
}

def _store_path_parts [
    store_path: string  # Nix store path or drv path to parse.
] {
    let parsed = (
        $store_path
        | parse --regex '^/nix/store/(?<hash>[^-]+)-(?<store_name>.+)$'
    )

    if ($parsed | is-empty) {
        return {
            path: $store_path
            hash: null
            store_name: ($store_path | path basename)
            package: ($store_path | path basename)
            version: null
            is_drv: ($store_path | str ends-with ".drv")
        }
    }

    let row = ($parsed | first)
    let name = ($row.store_name | str replace --regex '\.drv$' '')
    let versioned = ($name | parse --regex '^(?<package>.+?)-(?<version>[0-9].*)$')
    let package = if ($versioned | is-empty) { $name } else { $versioned.0.package }
    let version = if ($versioned | is-empty) { null } else { $versioned.0.version }

    {
        path: $store_path
        hash: $row.hash
        store_name: $name
        package: $package
        version: $version
        is_drv: ($store_path | str ends-with ".drv")
    }
}

def _as_path_list [] {
    let input = $in
    let kind = ($input | describe)

    if $kind == "nothing" {
        []
    } else if ($kind =~ '^(list|table)') {
        $input
    } else {
        [ $input ]
    }
}

# Parse Nix store paths or drv paths into readable package/version columns.
def drv-info [
    path?: string  # Optional store path. If omitted, reads a path or list of paths from stdin.
] {
    let paths = if $path == null {
        $in | _as_path_list
    } else {
        [ $path ]
    }

    $paths
    | each {|store_path| _store_path_parts ($store_path | into string) }
    | select package version store_name is_drv path hash
}

def _ni_closure_info [
    root: path  # Store path whose closure should be inspected.
] {
    ^nix path-info -r --json-format 1 --json $root
    | from json
    | transpose path info
    | insert nar_size {|row| $row.info.narSize? | default 0 }
    | insert size {|row| $row.nar_size | into filesize }
    | insert signatures {|row| $row.info.signatures? | default [] }
    | insert signature_count {|row| $row.signatures | length }
    | insert ultimate {|row| $row.info.ultimate? | default false }
    | insert deriver {|row| $row.info.deriver? | default null }
    | insert parsed {|row| _store_path_parts $row.path }
    | insert package {|row| $row.parsed.package }
    | insert version {|row| $row.parsed.version }
    | insert store_name {|row| $row.parsed.store_name }
    | select package version size nar_size ultimate signature_count signatures deriver path store_name
}

def _ni_sort_by_size [] {
    $in | sort-by nar_size --reverse
}

def _duration-text-seconds [
    text: string  # Human duration text from a Nix build log.
] {
    let hours = try { $text | parse --regex '(?<n>\d+)\s+hours?' | get 0.n | into int } catch { 0 }
    let minutes = try { $text | parse --regex '(?<n>\d+)\s+minutes?' | get 0.n | into int } catch { 0 }
    let seconds = try { $text | parse --regex '(?<n>\d+)\s+seconds?' | get 0.n | into int } catch { 0 }

    ($hours * 3600) + ($minutes * 60) + $seconds
}

def _nix-log-time-hints [
    path: string  # Store path whose build log should be inspected.
] {
    let log = (^nix log $path | complete)

    if $log.exit_code != 0 {
        return {
            build_log_available: false
            known_phase_seconds: null
            build_time_hints: []
            build_time_note: "nix log unavailable; likely substituted or log was garbage-collected"
        }
    }

    let hints = (
        $log.stdout
        | lines
        | parse --regex '^(?<phase>\S+) completed in (?<duration>.+)$'
        | insert seconds {|row| _duration-text-seconds $row.duration }
    )
    let known_seconds = if ($hints | is-empty) { null } else { $hints | get seconds | math sum }

    {
        build_log_available: true
        known_phase_seconds: $known_seconds
        build_time_hints: $hints
        build_time_note: (if $known_seconds == null {
            "log available, but no phase duration lines were found"
        } else {
            "sum of explicit phase duration lines only; not guaranteed total build time"
        })
    }
}

# Nix rebuild diagnostics. Run with no args for examples, or `help ni` for subcommands.
def ni [] {
    [
        {
            command: "ni closure <root>"
            description: "Show closure store paths with signature and deriver metadata."
            example: "ni closure (gens | first).store_path"
        }
        {
            command: "ni local <root>"
            description: "Show likely locally built or locally materialized closure paths."
            example: "ni local (gens | first).store_path"
        }
        {
            command: "ni subst <root>"
            description: "Show closure paths that carry binary-cache signatures."
            example: "ni subst (gens | first).store_path"
        }
        {
            command: "ni unsigned <root>"
            description: "Show unsigned closure paths that are not marked ultimate."
            example: "ni unsigned (gens | first).store_path"
        }
        {
            command: "ni added-local <old> <new>"
            description: "Show paths newly added to <new> that look locally built."
            example: "ni added-local (gens | get 1.store_path) (gens | first).store_path"
        }
        {
            command: "ni deriver <path>"
            description: "Return the .drv that produced a store path."
            example: "ni deriver /nix/store/...-codex-0.142.2"
        }
        {
            command: "ni why <root> <path>"
            description: "Explain why <path> is in <root>'s closure."
            example: "ni why (gens | first).store_path /nix/store/...-icu4c-78.3"
        }
        {
            command: "drv-info"
            description: "Parse store paths or drv paths into package/version columns."
            example: "ni local (gens | first).store_path | get deriver | drv-info"
        }
        {
            command: "ni log-times"
            description: "Add opt-in build-log timing hints to selected rows or paths."
            example: "ni local (gens | first).store_path | first 20 | ni log-times"
        }
    ]
}

# Show closure store paths with signature and deriver metadata.
def "ni closure" [
    root: path  # Store path whose closure should be inspected.
] {
    _ni_closure_info $root | _ni_sort_by_size
}

# Show likely locally built or locally materialized closure paths.
def "ni local" [
    root: path  # Store path whose closure should be inspected.
] {
    _ni_closure_info $root
    | where signature_count == 0 and ultimate == true
    | _ni_sort_by_size
}

# Show closure paths that carry binary-cache signatures.
def "ni subst" [
    root: path  # Store path whose closure should be inspected.
] {
    _ni_closure_info $root
    | where signature_count > 0
    | _ni_sort_by_size
}

# Show unsigned closure paths that are not marked ultimate.
def "ni unsigned" [
    root: path  # Store path whose closure should be inspected.
] {
    _ni_closure_info $root
    | where signature_count == 0 and ultimate == false
    | _ni_sort_by_size
}

# Show paths newly added to the new root that look locally built.
def "ni added-local" [
    old: path  # Older root store path.
    new: path  # Newer root store path.
] {
    let old_paths = (^nix path-info -r $old | lines)

    _ni_closure_info $new
    | where $it.path not-in $old_paths
    | where signature_count == 0 and ultimate == true
    | _ni_sort_by_size
}

# Show paths newly added to the new root that carry binary-cache signatures.
def "ni added-subst" [
    old: path  # Older root store path.
    new: path  # Newer root store path.
] {
    let old_paths = (^nix path-info -r $old | lines)

    _ni_closure_info $new
    | where $it.path not-in $old_paths
    | where signature_count > 0
    | _ni_sort_by_size
}

# Return the .drv that produced a store path.
def "ni deriver" [
    path: path  # Store path to query.
] {
    { path: $path, deriver: (^nix-store -q --deriver $path) }
}

# Explain why a path is in a closure.
def "ni why" [
    root: path  # Closure root.
    path: path  # Store path to explain.
] {
    ^nix why-depends $root $path
}

# Add build-log timing hints to selected `ni` rows or store paths.
def "ni log-times" [] {
    $in
    | _as_path_list
    | par-each --keep-order --threads 8 {|item|
        let kind = ($item | describe)
        let row = if ($kind =~ '^record') {
            $item
        } else {
            _store_path_parts ($item | into string)
        }
        let path = if ($kind =~ '^record') { $item.path } else { $item | into string }

        $row | merge (_nix-log-time-hints $path)
    }
}

def prompts [] {
    crdt | get chats | get original_command
}

# Set up theme-based environment variables and colors
setup_theme


# Update btop theme based on system theme (lazy-loaded)
# Call manually with: update-btop-theme


# Configure explore with Catppuccin Mocha palette — set fg+bg on every style
# so it renders correctly on both light and dark terminals.
$env.config.explore = {
    try: {
        reactive: true
    }
    status_bar_background: { fg: "#cdd6f4", bg: "#313244" }
    status_bar_text: { fg: "#cdd6f4", bg: "#313244" }
    command_bar_text: { fg: "#cdd6f4", bg: "#1e1e2e" }
    highlight: { fg: "#1e1e2e", bg: "#f9e2af" }
    status: {
        error: { fg: "#1e1e2e", bg: "#f38ba8" }
        warn:  { fg: "#1e1e2e", bg: "#f9e2af" }
        info:  { fg: "#1e1e2e", bg: "#89b4fa" }
    }
    selected_cell:   { fg: "#1e1e2e", bg: "#f9e2af" }
    selected_row:    { fg: "#cdd6f4", bg: "#313244" }
    selected_column: { fg: "#cdd6f4", bg: "#313244" }
    table: {
        split_line:    { fg: "#585b70", bg: "#1e1e2e" }
        cursor_row:    { fg: "#cdd6f4", bg: "#313244" }
        cursor_column: { fg: "#cdd6f4", bg: "#313244" }
        cursor_cell:   { fg: "#1e1e2e", bg: "#f9e2af" }
        line_head_top:    { fg: "#585b70", bg: "#1e1e2e" }
        line_head_bottom: { fg: "#585b70", bg: "#1e1e2e" }
        line_shift:       { fg: "#585b70", bg: "#1e1e2e" }
        line_index:       { fg: "#585b70", bg: "#1e1e2e" }
        show_head: true
        show_index: true
    }
}

def packages-last [] {
    packages | select name manifest_path | insert dir { $in.manifest_path | path dirname } | insert modified { git-files $in.dir | get modified | math max } | reject dir manifest_path | sort-by modified
}

def _symlink-entry [path: path] {
    let row = (try { ls -laD $path | first } catch { null })
    if $row == null or $row.type != symlink {
        null
    } else {
        let target = $row.target
        let resolved = if (($target | path split | first) == "/") {
            $target | path expand --no-symlink
        } else {
            $path | path dirname | path join $target | path expand --no-symlink
        }

        {
            path: ($path | path expand --no-symlink)
            target: $target
            resolved: $resolved
        }
    }
}

def _symlink-chain [path: path] {
    mut rows = []
    mut current = ($path | path expand --no-symlink)
    mut seen = []
    mut hop = 0

    loop {
        if $current in $seen {
            break
        }
        $seen = ($seen | append $current)

        let row = (_symlink-entry $current)
        if $row == null {
            break
        }

        $rows = ($rows | append ($row | insert hop $hop))
        $current = $row.resolved
        $hop = ($hop + 1)
    }

    $rows
}

def symlink-trail [
    path: path = "."  # Path to inspect.
] {
    let input_path = ($path | path expand --no-symlink)
    let parts = ($input_path | path split)
    mut rows = []

    for idx in 0..(($parts | length) - 1) {
        let component = ($parts | first ($idx + 1) | path join)
        let component_rows = (_symlink-chain $component | each {|row|
            $row | insert component $component
        })
        $rows = ($rows | append $component_rows)
    }

    $rows | enumerate | each {|row|
        $row.item | insert step $row.index
    } | select step component hop path target resolved
}

$env.config.table.missing_value_symbol = ""
$env.config.footer_mode = "never"  # Only show header at top
$env.config.table.abbreviated_row_count = 3


# https://github.com/nushell/nushell/issues/5552#issuecomment-2113935091
let abbreviations = abbr flatten-abbreviations {



    "..": "up"
    "ga.": "git add ."
    "gan.": "git add -N ."
    "gco.": "git checkout ."
    "gdm-": "git diff $'origin/(git_main_branch)...' --"
    "rr.": "rustrover ."
    # bt: "btop"
    # cs: "cargo shear"
    # cu: "cursor-agent --force"
    # gf: "git commit --fixup"
    # gf: "git-files"
    # gls: "git log --stat --stat-count=10 --"
    # gpr: "gt pr"
    # ip: "http ipapi.co/json"
    # ip: "ix "
    # or: "open --raw"
    # pl: "packages-last"
    # to: "http get :3001/tools"
    # up: "git fetch origin main:main && git rebase main && git push --force-with-lease"
    # xl: "x login"
    # xlo: "x login"
    # xls: "x ls"
    # xrm: "x rm"
    # xsh: "x ssh"
    # xss: "x sync -w"
    # y: "yazi"
    R: "cargo run"
    al: "aerospace list-windows --workspace focused --json | from json"
    am: "crdt | get chats | values | first | last"
    b: "bat"
    bb: "buck2 build '//...'"
    bc: "bacon clippy"
    bd: "bun dev"
    bda: "bun dev:api"
    bdm: "bun dev:monitor"
    bi: "bun install"
    br: "buck2 run"
    bri: "brew install"
    brl: "bun run lint"
    bs: "brew search"
    bt: "btop"
    cF: "cargo fetch"
    ca: "cat"
    cad: "cargo add"
    cb: "cargo build"
    cbb: "cargo build --release"
    cbi: "cargo binstall -y"
    cbp: "cargo build -p"
    cc: "c ~/Projects/indexable-inc/index"
    ccl: "cargo clean"
    cdo: "cargo doc --open"
    cdp: "^open -na 'Google Chrome' --args --remote-debugging-port=9222 --user-data-dir=($env.HOME)/chrome-cdp-profile"
    ce: "cargo run --example"
    cf: "cargo fmt"
    cft: "cloudflared tunnel --url http://localhost:8000"
    ci: "cargo install"
    cid: "cargo install --debug --locked --path"
    cig: "cargo install --git"
    cip: "cargo install --locked --path"
    # cl: "claude"
    gwl: "git worktree list"
    i1: "ix shell linux /run/current-system/sw/bin/compile bzImage"
    cld: "claude --debug"
    clh: "claude --model=haiku --dangerously-skip-permissions"
    cll: "claude --dangerously-skip-permissions --model=claude-opus-4-7 --effort xhigh"
    clp: "cargo clippy --all-targets  --timings -Zbuild-analysis"
    clpf: "cargo clippy --fix --all-targets --allow-dirty --allow-staged"
    clr: "claude --dangerously-skip-permissions --resume"
    cls: "claude --dangerously-skip-permissions --model=sonnet"
    clt: "claude --teleport"
    cm: "cargo metadata --format-version=1 | from json"
    cmn: "cargo metadata --no-deps --format-version=1 | from json"
    cnl: "cargo new --lib"
    hm: {
        base: "home-manager"
        children: {
            g: "generations"
            s: "switch --flake ~/.config/nix"
        }
    }
    co: "codex --yolo"
    cod: "pnpx @openai/codex --yolo"
    col: "columns"
    com: "complete"
    cone: "config env"
    conn: "config nu"
    cor: "codex --yolo resume"
    cpp: "realpath . | pbcopy"
    cpu: "ps -l | sort-by cpu"
    cr: "cargo run"
    cre: "cargo run --example"
    crp: "cargo run -p"
    crr: "cargo run --release"
    cs: "cargo search"
    ct: "cargo nextest run"
    cti: "cargo tree -i"
    ctr: "cargo tree"
    ctt: "cargo nextest run --release"
    cx: "chmod +x"
    d: "defer"
    db: "docker build ."
    dbi: "devbox-init"
    dbp: "docker build --platform linux/amd64 ."
    dbs: "devbox shell"
    dc: "detect columns --guess"
    dcd: "docker compose down"
    dcu: "docker compose up --build --watch"
    ddb: "duckdb -c"
    dl: "c ~/Downloads"
    dps: "docker ps"
    dr: "darwin-rebuild switch --flake ~/.config/nix"
    drb: "darwin-rebuild build --flake ~/.config/nix"
    drs: "darwin-rebuild switch --flake ~/.config/nix"
    dy: "bat -l yaml --style=plain"
    e: "explore -ip | to json -r "
    et: "eza --tree --icons=auto --git --git-ignore"
    ez: "eza --icons=auto --group-directories-first"
    cl: "claude"
    f1: "first"
    f2: "first 2"
    f3: "first 3"
    f4: "first 4"
    f5: "first 5"
    f6: "first 6"
    f7: "first 7"
    f8: "first 8"
    f9: "first 9"
    f: "fd -H"
    fj: "from json"
    fjo: "from json -o"
    fs: "from ssv"
    ft: "from tsv"
    g: "c ~/Projects/greenfield"
    ga: "git add"
    gan: "git add -N"
    gap: "git add -p"
    gb: "git branch"
    gba: "git branch -a"
    gbm: "git blame $'(git_main_branch)' --" # git log from main
    gbr: "gh browse"
    gc: "git commit"
    gca: "git commit --amend"
    gcl: "gix clone"
    gclb: "gix clone --bare"
    gcn: "git commit --no-verify"
    gco: "git checkout"
    gcom: "git checkout main --"
    gcp: "git add . ; git commit ; git push"
    gcs: "git commit --amend --no-edit -S"
    gd: "git diff"
    gdh: "git diff HEAD"
    gdm: "git diff $'origin/(git_main_branch)...'"
    gdms: "git diff $'origin/(git_main_branch)...' --stat"
    gds: "git diff --staged"
    gdss: "git diff --staged --stat"
    gdt: "git difftool"
    gf: "gh repo fork"
    gfa: "git fetch --all --prune"
    zz: "git add . ; git commit -m z; git push"
    ggl: "gsutil ls"
    gha: "git rev-parse --short HEAD | pbcopy"
    ghe: "git rev-parse HEAD"
    gii: "gh issue create -e"
    gil: "gh issue list --author @me"
    gl1: "git log -1" # beautiful git log with icons
    gl2: "git log -2" # beautiful git log with icons
    gl: "git log --oneline" # pretty ahead-of-main git log (index crate)
    id: "c index"
    gla: "git log --author='Andrew Gazelka'" # git log me
    glc: "glc" # git log conventional commits (styled)
    glg: "git lg --grep" # git log grep
    glm: "git lg $'(git_main_branch)...'" # git log from main
    glo: "git log --oneline"
    glp: "git log -p --ext-diff --"
    # gls: "gt ls"
    gls: "git log --stat --"
    # rl: "readlink"
    rl: "nix-direnv-reload"
    gmm: "git fetch --all; git merge $'origin/(git_main_branch)'"
    gp: "git push"
    gpf: "git push --force-with-lease"
    gpl: "git pull"
    gpm: "gh pr merge --auto --squash"
    gpp: "git push -u origin (git_current_branch)"
    gpr: "gh pr view -w"
    gprl: "gh pr list --author @me"
    gpro: "gh pr create -w"
    gprw: "gh pr view -w"
    gr: "git record"
    grb: "git rebase"
    grc: "git rebase --continue"
    gri: "git rebase -i --rebase-merges"
    grm: "git fetch --all; git rebase $'origin/(git_main_branch)'"
    gro: "c (git rev-parse --show-toplevel)"
    grom: "git rebase --onto main"
    grs: "git reset --soft"
    gs: "git status"
    gsa: "git stash"
    gsd: "git stash drop"
    gsho: "git show --ext-diff"
    gshos: "git show --stat"
    gsl: "git sl"
    gsm: "gt submit --publish --merge-when-ready --no-edit"
    gsmi: "git submodule init" # First time setting up a submodule that already exists in the repo
    gsms: "git submodule status"
    gsmu: "git submodule update" # actually download the files
    gsmui: "git submodule update --init"
    gsmuir: "git submodule update --init --recursive"
    gsp: "git stash pop"
    gss: "gt sync -a --force"
    gtc: "gt create -a"
    gwa: "git worktree add"
    gwc: "git switch -c"
    gwm: "gw main"
    gwp: "git switch -"
    hg: "http get --content-type=application/json",
    hj: "http --content-type=json"
    hp: "http post -e --full --content-type=application/json",
    hs: "http get :3001/tools/states"
    ht: "http"
    i: "ix"
    ill: "ix ls -l"
    ir: "ix rm --force"
    is: "ix shell"
    il: "ix ls"
    io: "crdt | get chats | values | first | where role == toolExecution | where name? == nushell | select input.command? output?.model | rename input output | update input { $in | nu-highlight }"
    iu: "ix up"
    jb: "jj bookmark set"
    jbm: "jj bookmark set main"
    jf: "just frontend-run"
    jp: "jj git push"
    jr: "just app-run"
    js: "jj status"
    k9: "k9s" # Kube
    k: "kill"
    kg: "kubectl get"
    kgsa: "kubectl get serviceaccounts"
    kp: "kubectl get pods -o json" # kbernetes pods
    l1: "last"
    l2: "last 2"
    l3: "last 3"
    l4: "last 4"
    l5: "last 5"
    l6: "last 6"
    l7: "last 7"
    l8: "last 8"
    l9: "last 9"
    l: "eza --icons=auto --group-directories-first"
    lag: "open ~/.superglide/logs/ai-gateway.log.jsonl| into datetime timestamp | sort-by timestamp | where timestamp > (date now) - 10min"
    le: "length"
    ll: "eza --icons=auto -a --group-directories-first"
    lso: "ls | sort-by modified"
    lt: "eza --icons=auto --sort=newest"
    mcp: "pnpx @modelcontextprotocol/inspector node build/index.js"
    msl: "mutagen sync list"
    msm: "mutagen sync monitor"
    ne: "nix eval --json"
    mst: "mutagen sync terminate"
    n: "nom"
    nb: "nom build"
    nd: "nom develop"
    nr: "nix run"
    ns: "nix search nixpkgs"
    o: "open"
    ocr: "uvx --from marker-pdf --with psutil marker_single gpro.pdf --output_dir ./output"
    of: "onefetch"
    ol: "orb list"
    oo: "open ."
    op: "open"
    or: "orb restart"
    ot: "open ~/.superglide/tools/**/*.json | update finished {|row| $row.finished | into datetime}  | sort-by finished  | where ((date now) - $it.finished) <= 10min"
    p: "pwd"
    lo: "lsof -i :"
    pa: "plugin add"
    pab: "pnpm approve-builds"
    pb: "pnpm build"
    pc: "PC_PORT_NUM=8081 process-compose --theme 'Custom Style'"
    pci: "pre-commit install"
    pcu: "process-compose up --tui=false --no-server --theme 'Custom Style'"
    pd: "pnpm dev"
    nfm: "nix flake metadata --json  | from json"
    pi: "pnpm install"
    pk: "pkill -9"
    pl: "git pull"
    pr: "c ~/Projects"
    psr: 'ps -l | where status == "Running"'
    pt: "ptree"
    pu: "pulumi"
    pud: "pulumi down --yes"
    puu: "pulumi up --yes"
    px: "pnpx"
    r: "rg --hidden"
    rf: "rm -rf"
    rj: "reject"
    ro: "railway open"
    in: "ix new"
    rp: "realpath"
    rr: "rustrover"
    s: "ssh"
    shc1: "ssh hil-compute-1"
    shc2: "ssh hil-compute-2"
    sai: "c searchai"
    sb: "sort-by"
    sc: "ssh codex"
    scl: "ssh claude"
    se: "select"
    # sh: "ssh -t hetzner"
    sm: "ssh main"
    so: "ssh codex"
    sor: "sort-by -r"
    sqlp: "SQLX_OFFLINE=false cargo sqlx prepare -- --all-features --tests --all-targets"
    sqlr: "cargo sqlx database reset -y"
    st: "speedtest-go"
    su: "git submodule update" # actually download the files
    sup: "c superglide"
    t: "trash"
    ta: "tmux attach -t" # open tasks
    tk: "tmux kill-session -t"
    tc: "typst compile"
    tj: "to json"
    tjr: "to json -r"
    tn: "to nuon"
    ty: "to yaml"
    u: "uniq -c"
    ua: "uv add"
    uenv: "uv venv"
    ul: "uv pip list"
    us: "uv sync --all-packages"
    v: "vim"
    vd: "viddy -t 'DFT_COLOR=always git diff origin/staging'"
    w: "where"
    wa: "which -a"
    wd: "pnpx wrangler deploy -e production"
    wg: "wget"
    wn: "where name"
    wr: "pnpx wrangler"
    wt: "pnpm wrangler tail --format json -e production" # wrangler tail
    z: "pbcopy"
    zi: "zoxide query -i"
    zj: "zellij"
    zja: "zellij a"
}

# Anywhere abbreviations - expand as arguments anywhere in the command
let anywhere_abbreviations = abbr flatten-abbreviations {
  LJ: "--log-format internal-json"
  H: "--help"
    # h: "--help"
    # v: "--version"
    # f: "--flatten"
}

# Helper for abbreviation expansion (wraps the testable abbr module)
# If skip_if_placeholder is true, abbreviations containing % won't be expanded
let abbr_expand_impl = {|buffer: string, skip_if_placeholder: bool|
    if $skip_if_placeholder {
        abbr expand $buffer $abbreviations $anywhere_abbreviations --skip-placeholders
    } else {
        abbr expand $buffer $abbreviations $anywhere_abbreviations
    }
}

# Load cargo completions
source completions.nu

# Closers for autopair tab-jump functionality
let autopair_closers = ['"' "'" ')' ']' '}' '`']

# IntelliJ-style bracket autopair. Typing an opener inserts the matching pair
# with the cursor between them; typing a closer when it's already the next char
# "types over" it (jumps past) instead of inserting a duplicate. Driven from the
# REPL via `commandline`, whose get-cursor/set-cursor are both grapheme-indexed
# (verified in nu-cli commandline/{get,set}_cursor.rs), so `str substring $c..$c`
# is the char just ahead of the cursor.
def autopair-open [open: string, close: string] {
    let line = (commandline)
    let cur = (commandline get-cursor)
    let next = ($line | str substring $cur..$cur)
    # Skip the auto-close when it would strand a closer in front of text,
    # matching how IDEs only auto-close before whitespace/closers/end-of-line.
    if ($next == "" or ($next in [")" "]" "}" " " (char tab)])) {
        commandline edit --insert ($open + $close)
        commandline set-cursor ($cur + 1)
    } else {
        commandline edit --insert $open
    }
}

def autopair-close [close: string] {
    let line = (commandline)
    let cur = (commandline get-cursor)
    let next = ($line | str substring $cur..$cur)
    if ($next == $close) {
        commandline set-cursor ($cur + 1)  # type over the existing closer
    } else {
        commandline edit --insert $close
    }
}

# Symmetric autopair for quote characters, where one key both opens and closes.
# Type-over wins: if the next char is already this quote, jump past it. Otherwise
# insert a pair when that won't strand a quote in front of text (same rule as
# autopair-open), else fall back to a single literal quote.
def autopair-quote [quote: string] {
    let line = (commandline)
    let cur = (commandline get-cursor)
    let next = ($line | str substring $cur..$cur)
    if ($next == $quote) {
        commandline set-cursor ($cur + 1)  # type over the existing quote
    } else if ($next == "" or ($next in [")" "]" "}" " " (char tab)])) {
        commandline edit --insert ($quote + $quote)
        commandline set-cursor ($cur + 1)
    } else {
        commandline edit --insert $quote
    }
}

let fish_completer = {|spans|
    XDG_CONFIG_HOME=/tmp/.config fish --command $"complete '--do-complete=($spans | str replace --all "'" "\\'" | str join ' ')'"
    | from tsv --flexible --noheaders --no-infer
    | rename value description
    | update value {|row|
      let value = $row.value
      let need_quote = ['\' ',' '[' ']' '(' ')' ' ' '\t' "'" '"' "`"] | any {$in in $value}
      if ($need_quote and ($value | path exists)) {
        let expanded_path = if ($value starts-with ~) {$value | path expand --no-symlink} else {$value}
        $'"($expanded_path | str replace --all "\"" "\\\"")"'
      } else {$value}
    }
}

# Uses nix's native NIX_GET_COMPLETIONS protocol: line 1 is the completion
# type (`normal`/`filenames`/`attrs`); lines 2+ are tab-separated value+desc.
# The env var holds the 1-indexed argument position being completed.
let nix_completer = {|spans|
    let pos = ($spans | length) - 1
    let raw = with-env {NIX_GET_COMPLETIONS: ($pos | into string)} {
        ^nix ...($spans | skip 1) | complete
    }
    if $raw.exit_code != 0 { return [] }
    $raw.stdout
    | lines
    | skip 1
    | where { |line| $line != "" }
    | each { |line|
        let row = ($line | split column "\t" value description | get 0)
        { value: $row.value, description: ($row.description? | default "") }
    }
}

let external_completer = {|spans|
    match $spans.0 {
        # nom (nix-output-monitor) is a drop-in for `nix build`/`develop`/`shell`;
        # nix_completer hardcodes `^nix` and drops the command name, so `nom build`
        # gets the exact `nix build` completions.
        "nix" | "nom" => (do $nix_completer $spans)
        _ => null
    }
}

$env.config = {
    # Shell integration controls terminal features via escape sequences
    shell_integration: {
        osc2: false    # Window/tab title updates - DISABLED to allow manual title changes
        osc7: true     # Current working directory reporting to terminal
        osc8: true     # Clickable hyperlinks in terminal output
        osc9_9: false  # ConEmu-specific integration (not needed for Ghostty)
        osc133: true   # VS Code shell integration (prompt markers)
        osc633: true   # VS Code command tracking and decorations
        reset_application_mode: true  # Reset terminal modes between commands
    }
    edit_mode: vi
    use_kitty_protocol: true  # Better key handling
    # system-clipboard feature enabled via cargo install --features system-clipboard
    history: {
        max_size: 100000
        sync_on_enter: false
        file_format: "sqlite"  # SQLite required for isolation
        isolation: true  # This enables per-session history
    }
    cursor_shape: {
        vi_insert: line
        vi_normal: block
    }



    # Menu configuration for tab completion
    show_banner: false

    # datetime_format: {
    #     table: "%H:%M:%S"
    # }

    hooks: {
        display_output: { render-explore-output }
        env_change: {
            PWD: [{||
                if (which direnv | is-not-empty) {
                    direnv export json | from json | default {} | load-env
                    if ($env.PATH | describe) == "string" {
                        $env.PATH = $env.PATH | split row (char esep)
                    }
                }
                github-pr-prompt-refresh-current-if-stale
            }]
        }
        pre_execution: [
            # (kv universal-variable-hook)  # disabled - causes errors on empty store
        ]
        # Ring bell when command finishes - shows ! in tmux for other windows
        pre_prompt: [{||
            github-pr-prompt-refresh-current-if-stale
            if "TMUX" in $env { print -n (char bel) }
        }]
    }

    # Keybindings for Fish-like tab completion
    keybindings: [
        # Ctrl-L (and Ghostty's Cmd-K, which sends \x0c) should drop scrollback
        # too, not just clear the viewport. Reedline's default ClearScreen emits
        # ESC[H ESC[2J, which leaves scrollback behind; ClearScrollback also
        # emits ESC[3J. See ghostty-org/ghostty#10288 for why the terminal sends
        # form-feed and defers the actual clear policy to the shell.
        {
            name: clear_scrollback
            modifier: control
            keycode: char_l
            mode: [emacs, vi_normal, vi_insert]
            event: { send: clearscrollback }
        }
        # Abbreviation expansion on enter (skips % placeholder abbreviations)
        {
            name: abbr_enter
            modifier: none
            keycode: enter
            mode: [emacs, vi_normal, vi_insert]
            event: [
                { send: menu name: abbr_menu_enter }
                { send: enter }
            ]
        }
        # Abbreviation expansion on space with % cursor positioning
        # PERF: Use insertchar (native) instead of executehostcommand when possible
        {
            name: abbr_space
            modifier: none
            keycode: space
            mode: [emacs, vi_normal, vi_insert]
            event: [
                { send: menu name: abbr_menu }
                { edit: insertchar value: ' ' }
            ]
        }
        # Separate binding to handle % placeholder positioning (Ctrl+Space)
        # Use this after typing an abbreviation that has a % placeholder
        {
            name: abbr_position_placeholder
            modifier: control
            keycode: space
            mode: [emacs, vi_normal, vi_insert]
            event: {
                send: executehostcommand
                cmd: "
                    let line = (commandline)
                    let pos = ($line | str index-of '%')
                    if $pos >= 0 {
                        commandline edit --replace ($line | str replace '%' '')
                        commandline set-cursor $pos
                    }
                "
            }
        }
        # {
        #     name: fuzzy_file
        #     modifier: control
        #     keycode: char_t
        #     mode: [emacs, vi_normal, vi_insert]
        #     event: {
        #         send: executehostcommand
        #         cmd: "commandline edit --insert (fzf --layout=reverse --height=40%)"
        #     }
        # }
        # System clipboard keybindings (require nushell compiled with system-clipboard feature)
        # Uncomment if your nushell build supports it:
        # {
        #     name: copy_selection_system
        #     modifier: control_shift
        #     keycode: char_c
        #     mode: [vi_normal, vi_insert]
        #     event: { edit: copyselectionsystem }
        # }
        # {
        #     name: yank_to_system
        #     modifier: none
        #     keycode: char_y
        #     mode: vi_normal
        #     event: { edit: copyselectionsystem }
        # }
        # {
        #     name: paste_from_system
        #     modifier: none
        #     keycode: char_p
        #     mode: vi_normal
        #     event: { edit: pastesystem }
        # }
        # {
        #     name: cut_line_to_system
        #     modifier: control_shift
        #     keycode: char_d
        #     mode: [vi_normal, vi_insert]
        #     event: [
        #         { edit: selectall }
        #         { edit: cutselectionsystem }
        #     ]
        # }
        # Alt+Backspace - delete word left (like emacs mode)
        {
            name: delete_word_left
            modifier: alt
            keycode: backspace
            mode: [emacs, vi_normal, vi_insert]
            event: { edit: backspaceword }
        }
        # Ctrl+U - delete to start of line (standard Unix binding)
        # Note: Cmd+Backspace doesn't work in terminals - Cmd is intercepted by macOS
        {
            name: delete_to_start
            modifier: control
            keycode: char_u
            mode: [emacs, vi_normal, vi_insert]
            event: { edit: cutfromstart }
        }
        # Ctrl+F - accept completion hint (like right arrow in fish)
        {
            name: complete_hint
            modifier: control
            keycode: char_f
            mode: [emacs, vi_normal, vi_insert]
            event: { send: historyhintcomplete }
        }
        # Ctrl+G - zoxide interactive fuzzy directory picker (sorted by frecency)
        # TODO: `c {x}` should match default selection here but doesn't - fzf's scoring
        # differs from zoxide's keyword algorithm. sk lacks --tiebreak=end so fzf required.
        # {
        #     name: zoxide_interactive
        #     modifier: control
        #     keycode: char_g
        #     mode: [emacs, vi_normal, vi_insert]
        #     event: {
        #         send: executehostcommand
        #         cmd: "let result = (_ZO_FZF_OPTS='--tiebreak=end' zoxide query -i | str trim); if ($result | is-not-empty) { cd $result; show_icons }"
        #     }
        # }
        # Autopair for double quotes - type " to insert "" with cursor in middle
        # If next char is already ", just move over it (char_u22 = " character)
        # {
        #     name: autopair_double_quote
        #     modifier: none
        #     keycode: char_u22
        #     mode: [emacs, vi_insert]
        #     event: {
        #         send: executehostcommand
        #         cmd: '
        #             let dq = (char dq)
        #             let line = (commandline)
        #             let cursor = (commandline get-cursor)
        #             let len = ($line | str length)
        #             let next_char = if $cursor < $len { $line | str substring $cursor..($cursor + 1) } else { "" }
        #             if $next_char == $dq {
        #                 commandline set-cursor ($cursor + 1)
        #             } else {
        #                 commandline edit --insert $"($dq)($dq)"
        #                 commandline set-cursor ((commandline get-cursor) - 1)
        #             }
        #         '
        #     }
        # }
        # Autopair for single quotes
        # {
        #     name: autopair_single_quote
        #     modifier: none
        #     keycode: char_u27
        #     mode: [emacs, vi_insert]
        #     event: {
        #         send: executehostcommand
        #         cmd: '
        #             let sq = (char sq)
        #             let line = (commandline)
        #             let cursor = (commandline get-cursor)
        #             let len = ($line | str length)
        #             let next_char = if $cursor < $len { $line | str substring $cursor..($cursor + 1) } else { "" }
        #             if $next_char == $sq {
        #                 commandline set-cursor ($cursor + 1)
        #             } else {
        #                 commandline edit --insert $"($sq)($sq)"
        #                 commandline set-cursor ((commandline get-cursor) - 1)
        #             }
        #         '
        #     }
        # }
        # Autopair for parentheses
        # {
        #     name: autopair_paren
        #     modifier: none
        #     keycode: char_u28
        #     mode: [emacs, vi_insert]
        #     event: {
        #         send: executehostcommand
        #         cmd: '
        #             let line = (commandline)
        #             let cursor = (commandline get-cursor)
        #             let len = ($line | str length)
        #             let next_char = if $cursor < $len { $line | str substring $cursor..($cursor + 1) } else { "" }
        #             if $next_char == ")" {
        #                 commandline set-cursor ($cursor + 1)
        #             } else {
        #                 commandline edit --insert "()"
        #                 commandline set-cursor ((commandline get-cursor) - 1)
        #             }
        #         '
        #     }
        # }
        # Autopair for brackets [ -> []
        # {
        #     name: autopair_bracket
        #     modifier: none
        #     keycode: char_u5b
        #     mode: [emacs, vi_insert]
        #     event: {
        #         send: executehostcommand
        #         cmd: '
        #             let line = (commandline)
        #             let cursor = (commandline get-cursor)
        #             let len = ($line | str length)
        #             let next_char = if $cursor < $len { $line | str substring $cursor..($cursor + 1) } else { "" }
        #             if $next_char == "]" {
        #                 commandline set-cursor ($cursor + 1)
        #             } else {
        #                 commandline edit --insert "[]"
        #                 commandline set-cursor ((commandline get-cursor) - 1)
        #             }
        #         '
        #     }
        # }
        # Autopair for braces { -> {}
        # {
        #     name: autopair_brace
        #     modifier: none
        #     keycode: char_u7b
        #     mode: [emacs, vi_insert]
        #     event: {
        #         send: executehostcommand
        #         cmd: '
        #             let line = (commandline)
        #             let cursor = (commandline get-cursor)
        #             let len = ($line | str length)
        #             let next_char = if $cursor < $len { $line | str substring $cursor..($cursor + 1) } else { "" }
        #             if $next_char == "}" {
        #                 commandline set-cursor ($cursor + 1)
        #             } else {
        #                 commandline edit --insert "{}"
        #                 commandline set-cursor ((commandline get-cursor) - 1)
        #             }
        #         '
        #     }
        # }
        # DISABLED: tab_smart breaks default tab completion behavior.
        # The event array runs BOTH executehostcommand AND completion_menu unconditionally.
        # nushell's `until` syntax can't help because executehostcommand doesn't signal success/failure.
        # The autopair keybindings already handle jump-over when typing the closing char directly.
        # {
        #     name: tab_smart
        #     modifier: none
        #     keycode: tab
        #     mode: [emacs, vi_insert]
        #     event: [
        #         { send: executehostcommand cmd: '
        #             let line = (commandline)
        #             let cursor = (commandline get-cursor)
        #             let len = ($line | str length)
        #             let next_char = if $cursor < $len { $line | str substring $cursor..($cursor + 1) } else { "" }
        #             if $next_char in $autopair_closers {
        #                 commandline set-cursor ($cursor + 1)
        #             }
        #         ' }
        #         { send: menu name: completion_menu }
        #     ]
        # }
    ]

    completions: {
        external: {
            enable: true
            # Router only handles `nix`; null falls back to nushell's defaults,
            # preserving prior behavior for every other command.
            completer: $external_completer
        }
        case_sensitive: true  # Case-insensitive for better fuzzy matching
        algorithm: fuzzy
        use_ls_colors: true
    }

    table: {
        mode: none
        index_mode: never
        header_on_separator: true
        padding: { left: 0, right: 1 }
    }

    menus: [
        # Abbreviation menu for space - expands all abbreviations including % placeholders
        {
            name: abbr_menu
            only_buffer_difference: false
            marker: none
            type: {
                layout: columnar
                columns: 1
                col_width: 20
                col_padding: 2
            }
            style: {
                text: green
                selected_text: green_reverse
                description_text: yellow
            }
            source: { |buffer, position| do $abbr_expand_impl $buffer false }
        }
        # Abbreviation menu for enter - skips % placeholder abbreviations
        {
            name: abbr_menu_enter
            only_buffer_difference: false
            marker: none
            type: {
                layout: columnar
                columns: 1
                col_width: 20
                col_padding: 2
            }
            style: {
                text: green
                selected_text: green_reverse
                description_text: yellow
            }
            source: { |buffer, position| do $abbr_expand_impl $buffer true }
        }
    ]
}

# IntelliJ-style bracket autopair keybindings, generated from one source of
# truth. keycodes are char_u<hex codepoint> (nu-cli reedline_config.rs parses
# this form). Bound only in insert/emacs modes so vi-normal motions (e.g. `(`,
# `}`) are untouched. Tab is intentionally left for completion: reedline can't
# make one key conditionally jump-or-complete (ExecuteHostCommand always exits
# and Menu always handles, so neither falls through an `until`), so the jump is
# the type-over above (press the closing bracket to move past it).
let autopair_pairs = [
    [open close open_key   close_key];
    ["("  ")"   "char_u28" "char_u29"]
    ["["  "]"   "char_u5b" "char_u5d"]
    ["{"  "}"   "char_u7b" "char_u7d"]
]
let autopair_keybindings = (
    $autopair_pairs | each { |p|
        [
            {
                name: $"autopair_open_($p.open_key)"
                modifier: none
                keycode: $p.open_key
                mode: [emacs vi_insert]
                event: { send: executehostcommand cmd: $'autopair-open "($p.open)" "($p.close)"' }
            }
            {
                name: $"autopair_close_($p.close_key)"
                modifier: none
                keycode: $p.close_key
                mode: [emacs vi_insert]
                event: { send: executehostcommand cmd: $'autopair-close "($p.close)"' }
            }
        ]
    } | flatten
)
$env.config.keybindings = ($env.config.keybindings | append $autopair_keybindings)

# Double-quote autopair: " is its own closer, so it gets the symmetric
# autopair-quote handler instead of the open/close pair above (char_u22 = ").
$env.config.keybindings = ($env.config.keybindings | append {
    name: autopair_quote_char_u22
    modifier: none
    keycode: char_u22
    mode: [emacs vi_insert]
    event: { send: executehostcommand cmd: "autopair-quote '\"'" }
})

# Tab opens the completion menu (nushell's default `until` chain), with Shift-Tab
# as a second binding for the same. We don't put the autopair closer tab-out on
# Tab: reedline's `Menu` event always handles the key (even with no matches) and
# `executehostcommand` exits the editor, so one key can't both complete and skip a
# closer. Typing the closer still types over it via autopair-close, which covers
# the common case.
$env.config.keybindings = ($env.config.keybindings | append [
    {
        name: completion_tab
        modifier: none
        keycode: tab
        mode: [emacs vi_insert]
        event: { until: [
            { send: menu name: completion_menu }
            { send: menunext }
            { edit: complete }
        ] }
    }
    {
        name: completion_shift_tab
        modifier: shift
        keycode: backtab
        mode: [emacs vi_insert]
        event: { until: [
            { send: menu name: completion_menu }
            { send: menuprevious }
        ] }
    }
])

# Starship prompt - init file should already exist from first run
# Regenerate manually if needed: starship init nu | save -f ~/.local/share/nushell/vendor/autoload/starship.nu
mkdir ($nu.data-dir | path join "vendor/autoload")
let starship_init = ($nu.data-dir | path join "vendor/autoload/starship.nu")
if not ($starship_init | path exists) {
    starship init nu | save -f $starship_init
}

let zoxide_init = ($nu.data-dir | path join "vendor/autoload/zoxide.nu")
if not ($zoxide_init | path exists) {
    zoxide init nushell | save -f $zoxide_init
}

github-pr-prompt-refresh-current-if-stale


alias vim = nvim
alias gs = git status
alias man = /usr/bin/man
alias tl = tail -n +1 -f


# # Platform-aware trash command (trash-cli on Linux, native trash on macOS)
# def --wrapped trash [...args] {
#     if (sys host).name == "Linux" {
#         ^trash-put ...$args
#     } else {
#         ^trash ...$args
#     }
# }

def --env mkd [dir: path] {
    mkdir $dir
    c $dir
}

# def vim [
#     path?: string@"nu-complete vim"
#     ...rest: string
# ] {
#     if ($path | is-empty) {
#         ^nvim ...$rest
#     } else {
#         ^nvim $path ...$rest
#     }
# }

alias v = vim

# Vim with completions from git diff/staging files
def vd [
    path?: string@"nu-complete git-diff-files"  # Changed files completion
    ...rest: string                              # Additional args
] {
    if ($path | is-empty) {
        ^nvim
    } else {
        vim $path ...$rest
    }
}

# # Trash wrapper with git-files completion and automatic glob expansion
# def trash [
#     path?: glob@"nu-complete git-files"  # First arg with git-tracked file completion and glob expansion
#     ...rest: glob                         # Additional glob patterns
# ] {
#     if ($path | is-empty) {
#         if ($rest | is-empty) {
#             ^trash
#         } else {
#             ^trash ...$rest
#         }
#     } else {
#         ^trash $path ...$rest
#     }
# }

# zoxide is auto-loaded from nushell/vendor/autoload/zoxide.nu

# Helper to recursively find files by extension (respects .gitignore)
def nu-complete-files-recursive [ext: string] {
    let clean_ext = if ($ext | str starts-with ".") {
        $ext | str substring 1..
    } else {
        $ext
    }
    fd --type f -e $clean_ext | lines
}

# Custom completer for vim using git-tracked files with fuzzy matching
def "nu-complete git-files" [] {
    if (git rev-parse --is-inside-work-tree | complete | get exit_code) == 0 {
        # Combine tracked and untracked (but not ignored) files
        # Put untracked FIRST since they're more likely what you want to edit
        let untracked = (git ls-files --others --exclude-standard | lines)
        let tracked = (git ls-files | lines | first 950)  # Leave room for untracked
        [$untracked $tracked] | flatten | uniq
    } else {
        # Use fd with limited depth for BFS behavior and limit results
        fd --type f --max-depth 5 --max-results 1000 | lines
    }
}

# Custom completer for files in git diff/staging
def "nu-complete git-diff-files" [] {
    if (git rev-parse --is-inside-work-tree | complete | get exit_code) == 0 {
        # Get all changed files: staged, unstaged, and untracked
        (git diff --name-only) + (git diff --cached --name-only) + (git ls-files --others --exclude-standard)
        | lines
        | uniq
    } else {
        []
    }
}

# Completer matching `git switch` (local + remote branches)
def "nu-complete git-branches" [] {
    if (^git rev-parse --is-inside-work-tree | complete | get exit_code) != 0 {
        return []
    }
    let local = (^git for-each-ref --format='%(refname:short)' refs/heads/ | lines)
    let remote = (
        ^git for-each-ref --format='%(refname:short)' refs/remotes/
        | lines
        | each { |b| $b | str replace --regex '^[^/]+/' '' }
        | where $it != "HEAD"
    )
    $local | append $remote | uniq
}

# Switch to a branch, jumping to its worktree if one exists
def --env gw [
    branch: string@"nu-complete git-branches"
    ...rest: string
] {
    let result = (^git worktree list --porcelain | complete)
    if $result.exit_code != 0 {
        ^git switch $branch ...$rest
        return
    }

    let entry = (
        $result.stdout
        | split row "\n\n"
        | where { |e| $e | lines | any { |line| $line == $"branch refs/heads/($branch)" } }
        | get 0?
    )

    if ($entry | is-empty) {
        ^git switch $branch ...$rest
    } else {
        let path = (
            $entry
            | lines
            | where { |line| $line | str starts-with "worktree " }
            | get 0
            | str replace "worktree " ""
        )
        cd $path
    }
    ^git pull --rebase
}

# Completer for .typ files
def "nu-complete typ-files" [] {
    nu-complete-files-recursive "typ"
}

# # Custom completer for vim: files/folders first, then nvim oldfiles (deduped)
# def "nu-complete vim" [] {
#     let files = try { ls | get name } catch { [] }
#     let recent = try {
#         ^nvim --headless -c 'lua io.write(table.concat(vim.v.oldfiles, "\n"))' -c 'q'
#         | lines
#         | where { $in | path exists }
#     } catch { [] }
#     let files_abs = $files | each { $in | path expand }
#     let recent_deduped = $recent | where { $in not-in $files_abs }
#     $files | append $recent_deduped
# }

def show_icons [] {
    eza --icons=auto --group-directories-first --sort=newest
}

# Run a closure in a background zellij pane (closes when done)
def defer [code: closure] {
    let source = view source $code
    zellij -s main run -c -- nu -c $"do ($source)"
}

# Completer for `c`. Nushell passes the line up to the cursor as `context`, so we
# read the token currently being typed: a path-like prefix (`~/...`, `./...`, `/`,
# or anything with a `/`) gets real directory completion, while a bare keyword
# falls back to the cwd's subdirs plus zoxide frecency. Returning the record form
# (`{options, completions}`) lets this completer pick its own matching rules and
# tag each candidate, independent of the global `$env.config.completions`. The
# path-like branch lets nushell prefix-filter; the keyword branch turns filtering
# off and matches the token itself (see there for why).
def "nu-complete c" [context: string] {
    let token = $context | str trim | split row ' ' | last
    let path_like = ($token =~ '^[~./]') or ($token | str contains '/')

    if $path_like {
        # Path candidates share the typed prefix, so prefix-match. Keep the typed
        # directory prefix verbatim so candidates still start with what the user
        # typed (e.g. `~/...`); a trailing slash means list that dir.
        let dir = if ($token | str ends-with '/') { $token } else { $token | path dirname }
        let completions = try {
            ls --short-names ($dir | path expand --no-symlink)
            | where type == dir
            | each {|row| { value: (($dir | path join $row.name) + '/'), description: dir } }
        } catch { [] }
        return { options: { sort: true, completion_algorithm: prefix, case_sensitive: false }, completions: $completions }
    }

    # Keyword branch: zoxide already does the fuzzy matching across the whole DB,
    # so disable nushell's re-filter (`filter: false`). Otherwise it would re-match
    # the typed token against our shortened display and drop most hits. We insert
    # the full absolute path (`value`, which `c` can always cd to) but show a
    # `display_override` relative to $PWD (or ~) so the menu doesn't repeat the
    # prefix we're already sitting in.
    # Canonicalize PWD: zoxide stores real paths, but $env.PWD isn't canonical, so
    # under a symlinked root (/tmp, /var) the relative form wouldn't fire otherwise.
    let pwd = ($env.PWD | path expand)
    let home = $nu.home-dir
    let shorten = {|p|
        if ($p == $pwd) { '.' } else if ($p | str starts-with ($pwd + '/')) {
            $p | str replace ($pwd + '/') ''
        } else if ($p == $home) { '~' } else if ($p | str starts-with ($home + '/')) {
            $p | str replace $home '~'
        } else { $p }
    }

    # cwd's own dirs, substring-filtered by the token ourselves (filter is off).
    let lc = ($token | str downcase)
    let local = (
        ls --short-names | where type == dir
        | where {|row| ($lc | is-empty) or ($row.name | str downcase | str contains $lc) }
        | each {|row| { value: ($row.name + '/'), description: dir } }
    )
    let local_names = ($local | get value)
    # zoxide frecency only makes sense with a query; an empty token would dump the
    # whole DB. Keep the top few so a short token doesn't flood the menu.
    # Capture with `complete`, not `try { zoxide | lines }`: a short token makes
    # zoxide emit >64KB, and an external streamed inside `try` deadlocks once its
    # output fills the pipe buffer. `complete` drains stdout fully and still lets
    # the outer `try` guard the case where zoxide isn't installed.
    let zox = if ($token | is-empty) { [] } else {
        let hits = try { zoxide query --list -- $token | complete | get stdout | lines } catch { [] }
        # Drop hits whose shortened display would duplicate a cwd dir already listed
        # above (both cd to the same place); filter is off so nothing else dedups.
        $hits | first 30
        | each {|p| { value: ($p + '/'), display_override: ((do $shorten $p) + '/'), description: zoxide } }
        | where {|z| $z.display_override not-in $local_names }
    }

    { options: { filter: false, sort: false, case_sensitive: false }, completions: ($local | append $zox) }
}

def --env c [...query: string@"nu-complete c"] {
    if ($query | is-empty) {
        cd ~
        show_icons
        return
    }
    # join so an unquoted dir with spaces (e.g. `c DJ Sets`) resolves as one path
    let joined = ($query | str join ' ')
    let expanded = ($joined | path expand)
    let target = if ($expanded | path exists) and ($expanded | path type) == 'dir' {
        $expanded
    } else {
        # pass words separately so zoxide ranks across all keywords
        ^zoxide query -- ...$query | str trim
    }
    cd $target
    show_icons
}

# alias '..'  = say hi

def --env up [] {
    cd ..
    show_icons
}


def --env '...' [] {
    cd ../..
    show_icons
}

# Typst compile with --open
def tc [
    path: string@"nu-complete typ-files"
    ...rest: string
] {
    ^typst compile $path --open ...$rest
}


# source ~/.local/share/atuin/init.nu

$env.config.color_config.string = {
    match $in {
        "TRACE" => "cyan"        # ANSI 36 - Cyan
        "DEBUG" | "started" => "blue"         # ANSI 34 - Blue
        "INFO" | "ok" => "green"               # ANSI 32 - Green
        "WARN" => "yellow"                     # ANSI 33 - Yellow
        "ERROR" => "red"                       # ANSI 31 - Red
        "nushell" => "purple"
        "search" => "light_cyan"
        "chrome_search" => "light_blue"
        "read" => "light_green"
        "scrape" => "light_yellow"
        "find_replace" => "light_red"
        # Git commit types
        "Feature" => "green"
        "Fix" => "red"
        "Docs" => "blue"
        "Style" => "magenta"
        "Refactor" => "yellow"
        "Perf" => "yellow"
        "Test" => "cyan"
        "Build" => "cyan"
        "CI" => "cyan"
        "Chore" => "dark_gray"
        _ => "default"  # default color
    }
}

$env.config.color_config.separator = "dark_gray"
$env.config.color_config.header = "#999937"
$env.config.color_config.filesize = "#89dceb"
$env.config.color_config.int = "default"
$env.config.color_config.float = "default"
$env.config.color_config.datetime = "purple"
$env.config.color_config.duration = "default"
$env.config.color_config.row_index = "dark_gray"







# use /Users/andrewgazelka/Projects/superglide/scripts/dev/init.nu *
# source /Users/andrewgazelka/Projects/superglide/scripts/shell-integration/superglide.nu

# Load Atuin shell history integration
source ($nu.default-config-dir | path join "atuin.nu")


def packages [] {
    cargo metadata --format-version=1 --no-deps | from json | get packages
}




def git-files [
    dir: string = ''       # Directory path or prefix filter
    --glob: glob           # Optional glob pattern to filter files (e.g., '*.rs', '**/*.toml')
] {
    # Build git ls-files command with optional glob pattern
    let git_args = if ($glob | is-empty) {
        if ($dir | is-empty) {
            ['-z']
        } else if ($dir | path expand | path exists) and ($dir | path type) == 'dir' {
            ['-z']
        } else {
            ['-z']
        }
    } else {
        # Git natively supports glob patterns
        ['-z' ($glob | into string)]
    }

    let files = if ($dir | is-empty) {
        # No directory specified, use current directory
        ^git ls-files ...$git_args
        | split row (char nul)
        | where $it != ''
    } else if ($dir | path expand | path exists) and ($dir | path type) == 'dir' {
        # Absolute or relative path to a directory
        let target_dir = $dir | path expand
        cd $target_dir
        ^git ls-files ...$git_args
        | split row (char nul)
        | where $it != ''
        | each { |file| $target_dir | path join $file }
    } else {
        # Treat as prefix filter within current repo
        ^git ls-files ...$git_args
        | split row (char nul)
        | where $it != '' and ($it starts-with $dir)
    }

    # Convert to ls output with metadata
    $files | par-each { ls $in } | flatten
}







# Table display settings
# Wrapping can cause buffering - using truncating instead
$env.config.table.trim = {
    methodology: wrapping
    wrapping_try_keep_words: false
    truncating_suffix: "..."
}


# Get all parent paths from a path
def "path parents" []: string -> list<string> {
    let parts = $in | path split
    1..(($parts | length) - 1)
    | each { |n| $parts | take $n | path join }
}

def group-max [group_field: string, max_field: string]: list<record> -> list<record> {
    $in
    | group-by --to-table { |row| $row | get $group_field }
    | each { |g|
        $g
        | update items { get $max_field | sort | last }
        | rename $group_field $max_field
    }
}

def tui-ports [] {
    open ~/.superglide/tui/*.port | lines  | flatten | into int
}

def tui-port [] {
    tui-ports | first
}


# List files with git blame info (last author + datetime), sorted by most recent
def lb [] {
    ls | each { |f|
        let info = (git log -1 --format="%an|%aI" -- $f.name | complete)
        if $info.exit_code == 0 {
            let parts = ($info.stdout | str trim | split row "|")
            {
                when: ($parts | get 1 | into datetime),
                name: $f.name,
                author: ($parts | get 0)
            }
        } else {
            { when: null, name: $f.name, author: "untracked" }
        }
    } | sort-by when
}

def --env y [...args] {
	let tmp = (mktemp -t "yazi-cwd.XXXXXX")
	^yazi ...$args --cwd-file $tmp
	let cwd = (open $tmp)
	if $cwd != "" and $cwd != $env.PWD {
		cd $cwd
	}
	rm -fp $tmp
}



# Start Claude Code in a new tmux session, named by the first argument when
# given; with no argument tmux picks its own session name.
def cl [session?: string] {
    if ($session | is-empty) {
        ^tmux new-session claude
    } else {
        ^tmux new-session -s $session claude
    }
}


# def sudo [ ...args:glob] {
#     mut sudo_args = $args
#
#     if (has_feature "sudo") {
#         # Extract just the sudo options (before the command)
#         let sudo_options = ($args | take until {|arg|
#             not (($arg | str starts-with "-") or ($arg | str contains "="))
#         })
#
#         # Prepend TERMINFO preservation flag if not using sudoedit
#         if (not ("-e" in $sudo_options or "--edit" in $sudo_options)) {
#             $sudo_args = ($args | prepend "--preserve-env=TERMINFO")
#         }
#     }
#
#     ^sudo ...$sudo_args
# }


alias cat = bat
