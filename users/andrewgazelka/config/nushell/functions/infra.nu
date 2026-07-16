const REMOTE_STATUS_PATH = path self ./infra/status.nu
const PROBE_OUTPUT_BLOCKS = 1_024
const PROBE_OUTPUT_LIMIT = 1MiB
const REMOTE_WRAPPER = '
/run/current-system/sw/bin/timeout --signal=TERM --kill-after=1s "$1" nu --no-config-file /dev/stdin
code=$?
case "$code" in
  0) exit 0 ;;
  124|137) exit 200 ;;
  *) printf "remote probe exited with code %s\n" "$code" >&2; exit 201 ;;
esac
'
const PROBE_WRAPPER = '
output_blocks=$1
stdout=$2
stderr=$3
status=$4
shift 4
if ! ulimit -f "$output_blocks"; then
  exit 70
fi
"$@" >"$stdout" 2>"$stderr"
code=$?
printf "%s\n" "$code" >"$status"
'

def "_infra error" [host: record, reachable: bool, message: string] {
    {
        host: $host.name
        reported_host: null
        ssh: $host.sshAlias
        reachable: $reachable
        generation: null
        activated: null
        revision: null
        system: null
        failed: null
        failed_units: []
        error: $message
    }
}

def "_infra status" [host: record, output: string] {
    let status = ($output | from json)
    let errors = [
        (if $status.host != $host.name {
            $"reported host ($status.host) does not match ($host.name)"
        })
        (if $status.active_generation != null and not $status.profile_matches_active {
            $"active generation ($status.active_generation) differs from profile generation ($status.profile_generation)"
        })
        (if $status.active_generation == null {
            "active system has no matching profile generation"
        })
    ] | compact

    {
        host: $host.name
        reported_host: $status.host
        ssh: $host.sshAlias
        reachable: true
        generation: $status.active_generation
        activated: (
            $status.activated * 1_000_000_000
            | into datetime
            | date to-timezone local
        )
        revision: $status.revision
        system: $status.system
        failed: ($status.failed_units | length)
        failed_units: $status.failed_units
        error: (if ($errors | is-empty) { null } else { $errors | str join "; " })
    }
}

def "_infra capped" [path] {
    try {
        (ls $path | get size | first) >= $PROBE_OUTPUT_LIMIT
    } catch {
        false
    }
}

# Show the active generation and failed systemd units across the NixOS fleet.
export def "infra ls" [
    --threads (-t): int = 8  # Maximum hosts queried concurrently.
    --timeout: duration = 8sec  # Timeout for connection setup and the remote probe.
] {
    if $threads < 1 {
        error make { msg: "threads must be positive" }
    }
    if $timeout <= 0sec {
        error make { msg: "timeout must be positive" }
    }

    let inventory = ($env.HOME | path join ".config" "infra" "hosts.json")
    let hosts = (open $inventory)
    let remote_status = (open --raw $REMOTE_STATUS_PATH)
    let timeout_seconds = ($timeout / 1sec | math ceil | into int)
    let watchdog_seconds = $timeout_seconds + 1
    let worker_count = if ($hosts | is-empty) {
        1
    } else {
        [$threads ($hosts | length)] | math min
    }
    let environment_options = ["-u" "LINEAR_API_KEY"]
    let ssh_options = [
        "-o" $"ConnectTimeout=($timeout_seconds)"
        "-o" "BatchMode=yes"
        "-o" "ClearAllForwardings=yes"
        "-o" "ForwardAgent=no"
        "-o" "ForwardX11=no"
        "-o" "LogLevel=ERROR"
        "-o" $"ServerAliveInterval=($timeout_seconds)"
        "-o" "ServerAliveCountMax=1"
    ]
    let quoted_remote_wrapper = $"'($REMOTE_WRAPPER)'"
    let remote_command = [
        "/bin/sh"
        "-c"
        $quoted_remote_wrapper
        "infra-status"
        $"($timeout_seconds)s"
    ]

    $hosts
    | par-each --keep-order --threads $worker_count { |host|
        let capture_directory = (^mktemp -d | str trim | path expand)
        let stdout_path = ($capture_directory | path join "stdout")
        let stderr_path = ($capture_directory | path join "stderr")
        let status_path = ($capture_directory | path join "status")
        let local_command = [
            "/bin/sh"
            "-c"
            $PROBE_WRAPPER
            "infra-probe"
            $PROBE_OUTPUT_BLOCKS
            $stdout_path
            $stderr_path
            $status_path
            "timeout"
            "--signal=TERM"
            "--kill-after=1s"
            $"($watchdog_seconds)s"
            "ssh"
            ...$ssh_options
            "--"
            $host.sshAlias
            ...$remote_command
        ]
        let wrapper = (
            $remote_status
            | do {
                ^env ...$environment_options ...$local_command
            }
            | complete
        )
        let result = {
            exit_code: (try {
                open --raw $status_path | str trim | into int
            } catch {
                null
            })
            stdout: (try { open --raw $stdout_path } catch { "" })
            stderr: (try {
                open --raw $stderr_path | str trim | str substring 0..<4_096
            } catch {
                ""
            })
            output_capped: ((_infra capped $stdout_path) or (_infra capped $stderr_path))
        }
        let reached_host = $result.exit_code in [0 200 201]
        rm --recursive --force $capture_directory

        if $result.exit_code == null {
            let detail = ($wrapper.stderr | str trim)
            let message = if ($detail | is-empty) {
                "local probe wrapper failed"
            } else {
                $"local probe wrapper failed: ($detail)"
            }
            _infra error $host false $message
        } else if $result.output_capped {
            _infra error $host false $"probe output exceeded ($PROBE_OUTPUT_LIMIT)"
        } else if $result.exit_code == 0 {
            let parsed = try {
                { row: (_infra status $host $result.stdout), error: null }
            } catch { |error|
                {
                    row: null
                    error: ($error.msg? | default "invalid probe response")
                }
            }
            if $parsed.error == null {
                $parsed.row
            } else {
                _infra error $host true $parsed.error
            }
        } else {
            let message = if $result.exit_code in [124 137] {
                $"ssh exceeded the ($watchdog_seconds)s local watchdog"
            } else if $result.exit_code == 200 {
                $"probe timed out after ($timeout)"
            } else if $result.exit_code == 201 {
                if ($result.stderr | is-empty) { "remote probe failed" } else { $result.stderr }
            } else if $result.exit_code in [125 126 127] {
                $"local probe command failed with code ($result.exit_code)"
            } else if ($result.stderr | is-empty) {
                $"ssh exited with code ($result.exit_code)"
            } else {
                $result.stderr
            }
            _infra error $host $reached_host $message
        }
    }
}
