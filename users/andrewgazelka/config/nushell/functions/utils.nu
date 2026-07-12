# Add to config.nu

# export def "from jsonl" [] {
#     from json --objects
# }


export def display-img [data: binary] {
    $data | encode base64 | $"\e_Ga=T,f=100;($in)\e\\" | print -n
    print ""  # Add newline after image
}

# Send desktop notification (macOS only for now)
# Commented out - using external notify from flake input instead
# def notify [
#     message: string
#     --title (-t): string = "Notification"
# ] {
#     if ($nu.os-info.name == "macos") {
#         ^osascript -e $"display notification \"($message)\" with title \"($title)\" sound name \"Glass\""
#     } else {
#         print $"[($title)] ($message)"
#     }
# }

# def "from log" [] {
#     bat --plain --language log
# }

# def "from wav" [] {
#     play -
# }
#
export def "from png" [] {
    display-img $in
}

export def "from jsonl" [] {
    from json -o
}
#
# def "from md" [] {
#     glow -p $in
# }

export def "tok length" []: string -> int {
    uvx ttok | complete | get stdout | into int
}

export def "tok cost sonnet input" []: int -> float {
    # Claude Sonnet 4 pricing: $3 per million input tokens
    let tokens = $in
    let price_per_million = 3.0

    $tokens / 1_000_000 * $price_per_million
}

export def "tok cost sonnet output" []: int -> float {
    # Claude Sonnet 4 pricing: $15 per million output tokens
    let tokens = $in
    let price_per_million = 15.0

    $tokens / 1_000_000 * $price_per_million
}


export def paths [] {
    $env.PATH | split row (char esep)
}

export def "path executables" [] {
    paths | each { ls -l $in} | flatten | where mode =~ x | get name | uniq
}

# def "from typ" [] {
#     typst compile $in --open
# }

export def "from toml.orig" [] {
    from toml
}

# def "from ndjson" [] {
#     from jsonl
# }

export def "from archive" [] {
    into binary
}

export def "into apple-timestamp" [
    --timezone (-t): string = 'America/Los_Angeles'  # timezone flag
] {
    $in / 1_000_000_000 | $in + 978307200 | into datetime --timezone $timezone
}

export def appl [] {
  # Apple nanoseconds to Unix nanoseconds
  # 978307200 seconds * 1_000_000_000 = nanoseconds between epochs
  $in + 978_307_200_000_000_000 | into datetime
}

# def "from plist" [] {
#     from xml --allow-dtd
# }

# Returns hierarchical tree structure with children nested
export def children [pid: int] {
    let direct_children = ps | where ppid == $pid
    if ($direct_children | is-empty) {
        []
    } else {
        $direct_children | par-each { |child|
            let child_children = children $child.pid
            $child | insert children $child_children
        }
    }
}

# Returns flattened list of all descendants
export def descendants [pid: int] {
    let direct_children = ps | where ppid == $pid
    if ($direct_children | is-empty) {
        []
    } else {
        let nested = $direct_children | par-each { |child|
            [$child] ++ (descendants $child.pid)
        }
        $nested | flatten
    }
}

export def ptree [pid: int] {
    # Get the full parent chain from child to root
    mut current_pid = $pid
    mut tree = []

    while $current_pid != 0 {
        let proc = (try { ps | where pid == $current_pid | first } catch { break })
        $tree = ($tree | append $proc)
        $current_pid = $proc.ppid
    }

    # Return sorted from most child to most parent (already in this order)
    $tree
}

export def prefixes [] {
  let input = $in
  1..($input | length) | each { |n|
    $input | first $n
  }
}

export def lowercase-columns [] {
    let input = $in
    let cols = $input | columns
    let new_cols = $cols | each { |col| $col | str downcase }
    $input | rename ...$new_cols
}

export def lo [pids: any] {
    let pid_list = if ($pids | describe) == "int" {
        [$pids]
    } else {
        $pids
    }

    let pid_args = $pid_list | str join ","
    lsof -P -p $pid_args | detect columns | lowercase-columns
    # | group-by pid --to-table | update items { reject pid }
}



export def colors [] {
	# Show basic 16 colors with names
	let all_colors = [
	    [0 "Black"]
	    [1 "Red"]
	    [2 "Green"]
	    [3 "Yellow"]
	    [4 "Blue"]
	    [5 "Magenta"]
	    [6 "Cyan"]
	    [7 "White"]
	    [8 "Bright Black"]
	    [9 "Bright Red"]
	    [10 "Bright Green"]
	    [11 "Bright Yellow"]
	    [12 "Bright Blue"]
	    [13 "Bright Magenta"]
	    [14 "Bright Cyan"]
	    [15 "Bright White"]
	]

	$all_colors | each {|c|
	    let fg = $"(ansi -e '38;5;')($c.0)m"
	    let bg = $"(ansi -e '48;5;')($c.0)m"
	    let reset = (ansi reset)
	    $"($fg)▌($reset)($bg)   ($reset) ($c.0 | fill -a r -w 2) ($c.1)"
	} | str join (char newline) | print
}


export def tokenize [path: string] {
    let bin = $"($env.HOME)/Projects/superglide/target/release/file-search"
    if ($bin | path exists) {
        ^$bin tokenize $path | from json -o
    } else {
        error make {msg: "file-search binary not found"}
    }
}

export def dedup [] {
    reduce -f [] {|it, acc|
        if ($acc | is-empty) or ($acc | last) != $it {
            $acc | append $it
        } else {
            $acc
        }
    }
}

# Update btop theme based on system dark mode
export def update-btop-theme [] {
    let btop_config = $"($env.HOME)/.config/btop/btop.conf"

    let theme = if ($nu.os-info.name == "macos") {
        let appearance = (defaults read -g AppleInterfaceStyle | complete)
        if $appearance.exit_code == 0 and ($appearance.stdout | str trim) == "Dark" {
            "dracula"
        } else {
            "gruvbox_light"
        }
    } else {
        # Linux: default to dark theme
        "dracula"
    }

    let config_content = open $btop_config
    let updated = $config_content | str replace 'color_theme = "[^"]*"' $'color_theme = "($theme)"'
    $updated | save -f $btop_config
}

# Copy file reference to clipboard like Cmd-C in Finder (macOS only)
export def cpfile [path: path] {
    if ($nu.os-info.name != "macos") {
        error make {msg: "cpfile is only supported on macOS"}
    }
    let absolute_path = $path | path expand
    let filename = $absolute_path | path basename
    # Use AppleScriptObjC with NSPasteboard to properly set file URL and filename
    let script = r#'use AppleScript version "2.4"
use framework "Foundation"
use framework "AppKit"

set theURL to current application's |NSURL|'s fileURLWithPath:"__PATH__"
set theClip to current application's NSPasteboard's generalPasteboard()
theClip's clearContents()
theClip's writeObjects:{theURL}
theClip's setString:"__FILENAME__" forType:(current application's NSPasteboardTypeString)
'# | str replace '__PATH__' $absolute_path | str replace '__FILENAME__' $filename
    ^osascript -e $script | ignore
}

# Download YouTube captions/subtitles
export def yt-captions [
    url: string              # YouTube video URL
    --lang (-l): string = "en"  # Subtitle language code (default: en)
] {
    yt-dlp --write-auto-sub --sub-lang $lang --skip-download -o "%(title)s" $url
}
