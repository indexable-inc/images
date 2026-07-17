use std/assert
use ../functions/infra.nu *

const STATUS_PROBE_PATH = path self ../functions/infra/status.nu

#[test]
def test_remote_status_probe [] {
    let directory = (^mktemp -d | str trim | path expand)
    let bin = ($directory | path join "bin")
    let profiles = ($directory | path join "profiles")
    let current_system = ($directory | path join "current-system")
    mkdir $bin $profiles
    touch $current_system
    for generation in [41 42 43] {
        touch ($profiles | path join $"system-($generation)-link")
    }
    touch ($profiles | path join "system")

    '#!/bin/sh
case "$*" in
  *"-f "*"system-41-link") printf "/nix/store/old-nixos-system-up\n" ;;
  *"-f "*"system-42-link") printf "/nix/store/abc-nixos-system-up\n" ;;
  *"-f "*"system-43-link") printf "/nix/store/new-nixos-system-up\n" ;;
  *"-f "*"current-system") printf "/nix/store/abc-nixos-system-up\n" ;;
  *"-f "*"profiles/system") printf "/nix/store/new-nixos-system-up\n" ;;
  *"profiles/system") printf "system-43-link\n" ;;
  *) exit 1 ;;
esac
' | save --raw ($bin | path join "readlink")
    '#!/bin/sh
printf "1700000000\n"
' | save --raw ($bin | path join "stat")
    '#!/bin/sh
printf "[%s]\n" "{\"unit\":\"broken.service\"},{\"unit\":\"other.service\"}"
' | save --raw ($bin | path join "systemctl")
    '#!/bin/sh
printf "0123456789abcdef\n"
' | save --raw ($bin | path join "sudo")
    '#!/bin/sh
printf "up\n"
' | save --raw ($bin | path join "hostname")
    for command in [readlink stat systemctl sudo hostname] {
        chmod +x ($bin | path join $command)
    }

    let original_path = $env.PATH
    $env.PATH = [$bin] ++ $original_path
    let status = (
        nu --no-config-file $STATUS_PROBE_PATH --profiles $profiles --current-system $current_system
        | from json
    )
    $env.PATH = $original_path

    assert equal $status.host "up"
    assert equal $status.active_generation 42
    assert equal $status.profile_generation 43
    assert not $status.profile_matches_active
    assert equal $status.activated 1_700_000_000
    assert equal $status.system "/nix/store/abc-nixos-system-up"
    assert equal $status.revision "0123456789abcdef"
    assert equal $status.failed_units ["broken.service" "other.service"]

    rm --recursive --force $directory
}

#[test]
def test_reachable_and_unreachable_hosts [] {
    let directory = (^mktemp -d | str trim | path expand)
    let bin = ($directory | path join "bin")
    let inventory_directory = ($directory | path join ".config" "infra")
    let response = ($directory | path join "response.json")
    mkdir $bin $inventory_directory

    '#!/bin/sh
cat >/dev/null
if [ -n "${LINEAR_API_KEY-}" ]; then
  printf "credential leaked\n" >&2
  exit 99
fi
case "$*" in
  *ClearAllForwardings=yes*ForwardAgent=no*ForwardX11=no*) ;;
  *) printf "unsafe ssh options\n" >&2; exit 98 ;;
esac
case "$*" in
  *"-- down "*) printf "connection refused\n" >&2; exit 255 ;;
  *"-- invalid "*) printf "not json\n"; exit 0 ;;
  *"-- timeout "*) exit 200 ;;
  *"-- slow "*) sleep 5; exit 0 ;;
  *"-- large "*) yes x | head -c 2097152; exit 0 ;;
esac
cat "$FAKE_INFRA_RESPONSE"
' | save --raw ($bin | path join "ssh")
    chmod +x ($bin | path join "ssh")

    {
        host: "up"
        active_generation: 42
        profile_generation: 42
        profile_matches_active: true
        activated: 1_700_000_000
        system: "/nix/store/abc-nixos-system-up"
        revision: "0123456789abcdef"
        failed_units: ["broken.service"]
    }
    | to json
    | save --raw $response

    [
        { name: "up", sshAlias: "up" }
        { name: "down", sshAlias: "down" }
        { name: "invalid", sshAlias: "invalid" }
        { name: "timeout", sshAlias: "timeout" }
        { name: "slow", sshAlias: "slow" }
        { name: "large", sshAlias: "large" }
    ]
    | to json
    | save --raw ($inventory_directory | path join "hosts.json")

    let original_home = $env.HOME
    let original_path = $env.PATH
    let original_linear_api_key = $env.LINEAR_API_KEY?
    $env.HOME = $directory
    $env.PATH = [$bin] ++ $original_path
    $env.FAKE_INFRA_RESPONSE = $response
    $env.LINEAR_API_KEY = "must-not-leak"
    let started = date now
    let rows = (infra ls --timeout 1sec)
    let elapsed = (date now) - $started
    $env.HOME = $original_home
    $env.PATH = $original_path
    if $original_linear_api_key == null {
        hide-env LINEAR_API_KEY
    } else {
        $env.LINEAR_API_KEY = $original_linear_api_key
    }

    assert equal ($rows | length) 6
    assert ($elapsed < 4sec)
    assert equal $rows.0.generation 42
    assert equal $rows.0.revision "0123456789abcdef"
    assert equal $rows.0.system "/nix/store/abc-nixos-system-up"
    assert equal $rows.0.failed_units ["broken.service"]
    assert not $rows.1.reachable
    assert str contains $rows.1.error "connection refused"
    assert $rows.2.reachable
    assert not ($rows.2.error | is-empty)
    assert $rows.3.reachable
    assert str contains $rows.3.error "timed out"
    assert not $rows.4.reachable
    assert str contains $rows.4.error "local watchdog"
    assert not $rows.5.reachable
    assert str contains $rows.5.error "probe output exceeded"

    rm --recursive --force $directory
}

export def run-all [] {
    test_remote_status_probe
    test_reachable_and_unreachable_hosts
}

def main [] {
    run-all
}
