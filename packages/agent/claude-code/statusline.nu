# House Claude Code statusline (settings `statusLine.command`, baked by
# package.nix into the wrapper's read-only settings layer). Renders one
# dark-gray line: context-window bar, model, effort level, and the running CLI
# version with an "↑<latest>" marker when Anthropic has published a newer
# release than the wrapper pins.
#
# Claude Code pipes a JSON status payload on stdin and re-runs this on every
# render, so everything here must be fast and fail-soft: the one network call
# (the `latest` version pointer) is cached for hours and swallowed on error.

# Anthropic's release bucket: `<base>/latest` is a plain-text version string,
# the same pointer update.nix tracks when bumping manifest.json.
const release_base = "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases"
const cache_ttl = 6hr

# Newest published version, from a small mtime-TTL cache so at most one
# render per TTL window pays the (2s-capped) fetch. Null when offline with a
# cold cache: the caller then renders the plain version, no marker.
def latest-version [] {
  let cache_root = ($env.XDG_CACHE_HOME? | default $"($env.HOME)/.cache")
  let cache_dir = $"($cache_root)/ix-claude-statusline"
  let cache_file = $"($cache_dir)/latest"

  let cached_fresh = (try {
    ((date now) - (ls $cache_file | first | get modified)) < $cache_ttl
  } catch { false })
  if $cached_fresh {
    return (open --raw $cache_file | str trim)
  }

  let fetched = (try {
    http get --max-time 2sec $"($release_base)/latest" | str trim
  } catch { "" })
  if ($fetched | is-not-empty) {
    mkdir $cache_dir
    $fetched | save --force $cache_file
    return $fetched
  }
  # Stale cache beats nothing while the network is away.
  try { open --raw $cache_file | str trim } catch { null }
}

# Numeric per-segment compare, so a pinned-ahead `next` build (local > latest)
# is not flagged as outdated the way plain string inequality would.
def is-newer [latest: string, current: string] {
  try {
    let l = ($latest | split row "." | each {|p| $p | into int })
    let c = ($current | split row "." | each {|p| $p | into int })
    let n = ([($l | length) ($c | length)] | math max)
    for i in 0..<$n {
      let a = (if $i < ($l | length) { $l | get $i } else { 0 })
      let b = (if $i < ($c | length) { $c | get $i } else { 0 })
      if $a > $b { return true }
      if $a < $b { return false }
    }
    false
  } catch { false }
}

# `--default-effort` carries the house `effortLevel` baked into the wrapper's
# read-only settings layer, which this script cannot read back from disk; the
# writable settings files below still win when the user overrides per-machine.
def main [--default-effort: string = ""] {
  let input = (open --raw /dev/stdin | from json)

  let model = ($input.model?.display_name? | default "?")

  # Effort cascade: settings.local.json > settings.json > baked default.
  let effort = (try {
    let local_settings = $"($env.HOME)/.claude/settings.local.json"
    let user_settings = $"($env.HOME)/.claude/settings.json"
    let from_local = (if ($local_settings | path exists) {
      open $local_settings | get effortLevel?
    } else { null })
    let from_user = (if ($user_settings | path exists) {
      open $user_settings | get effortLevel?
    } else { null })
    $from_local | default $from_user | default $default_effort
  } catch { $default_effort })

  # Context bar.
  let context_size = ($input.context_window?.context_window_size? | default 200000)
  let usage = $input.context_window?.current_usage?
  let ctx_pct = (if $usage != null {
    let total = (
      ($usage.input_tokens? | default 0)
      + ($usage.cache_creation_input_tokens? | default 0)
      + ($usage.cache_read_input_tokens? | default 0)
    )
    $total * 100 // $context_size
  } else { 0 })
  let bar_width = 10
  let filled = ($ctx_pct * $bar_width // 100 | [$in $bar_width] | math min)
  let empty = $bar_width - $filled
  let filled_str = (if $filled > 0 { 1..$filled | each { "█" } | str join } else { "" })
  let empty_str = (if $empty > 0 { 1..$empty | each { "░" } | str join } else { "" })
  let bar = $filled_str + $empty_str

  # Version, with an update marker when upstream has moved past this build.
  let current = ($input.version? | default "" | str trim)
  let version_segment = (if ($current | is-not-empty) {
    let latest = (latest-version)
    if $latest != null and (is-newer $latest $current) {
      $" | v($current) (ansi yellow)↑($latest)(ansi reset)(ansi dark_gray)"
    } else {
      $" | v($current)"
    }
  } else { "" })

  let effort_segment = (if ($effort | is-not-empty) { $" | ($effort)" } else { "" })

  print $"(ansi dark_gray)($bar) | ($model)($effort_segment)($version_segment)(ansi reset)"
}
