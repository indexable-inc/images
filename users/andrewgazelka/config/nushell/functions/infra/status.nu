def main [
    --profiles: path = "/nix/var/nix/profiles"
    --current-system: path = "/run/current-system"
] {
    let profile = ($profiles | path join "system")
    let profile_link = (^readlink $profile | str trim)
    let profile_generation = (
        $profile_link
        | path basename
        | str replace "system-" ""
        | str replace "-link" ""
        | into int
    )
    let profile_system = (^readlink -f $profile | str trim)
    let system = (^readlink -f $current_system | str trim)
    let active_generations = (
        glob ($profiles | path join "system-*-link")
        | where { |link| (^readlink -f $link | str trim) == $system }
        | each { |link|
            $link
            | path basename
            | str replace "system-" ""
            | str replace "-link" ""
            | into int
        }
    )
    let active_generation = if ($active_generations | is-empty) {
        null
    } else {
        $active_generations | math max
    }
    let activated = (
        ^stat -c "%Y" $current_system
        | str trim
        | into int
    )
    let failed_units = (
        ^systemctl list-units --failed --all --output=json --no-pager
        | from json
        | get unit
    )
    let revision_result = (
        do { ^sudo -n cat /var/lib/ix-deploy-record/last-rev }
        | complete
    )
    let revision = if $revision_result.exit_code == 0 {
        $revision_result.stdout | str trim
    } else {
        null
    }
    {
        host: (^hostname | str trim)
        active_generation: $active_generation
        profile_generation: $profile_generation
        profile_matches_active: ($profile_system == $system)
        activated: $activated
        system: $system
        revision: $revision
        failed_units: $failed_units
    } | to json --raw
}
