# Home Manager Style (HMS) - Cross-platform Nix configuration management
#
# Commands:
#   hms switch  - Apply home-manager + system config
#   hms build   - Build without applying
#   hms check   - Validate flake
#   hms update  - Update flake inputs
#   hms gc      - Garbage collect old generations
#   hms diff    - Show pending changes

const FLAKE_DIR = "~/.config/nix"

# Detect current platform
def get-platform [] {
    let os = (sys host | get name)
    if $os == "Darwin" {
        "darwin"
    } else if (which nixos-rebuild | is-not-empty) {
        "nixos"
    } else {
        "linux"
    }
}

# Get the hostname for config selection
def get-hostname [] {
    hostname | str trim
}

# Get home-manager config name
def get-hm-config [] {
    let user = $env.USER
    let host = get-hostname
    let platform = get-platform

    # Try specific first, fall back to generic
    let specific = $"($user)@($host)"
    let generic = match $platform {
        "darwin" => $"($user)@darwin"
        "nixos" => $"($user)@utm"
        _ => $"($user)@linux-arm"
    }

    # Check which exists in flake
    let flake_path = ($FLAKE_DIR | path expand)
    let check = (do { nix flake show $flake_path --json } | complete)

    if ($check.exit_code == 0) {
        let outputs = ($check.stdout | from json)
        let hm_configs = ($outputs | get -i homeConfigurations | default {} | columns)

        if ($specific in $hm_configs) {
            $specific
        } else if ($generic in $hm_configs) {
            $generic
        } else if ($user in $hm_configs) {
            $user
        } else {
            # Default fallback
            $user
        }
    } else {
        $user
    }
}

# Apply home-manager and system configuration
export def "hms switch" [] {
    let platform = get-platform
    let flake = ($FLAKE_DIR | path expand)

    print $"(ansi cyan)Platform: (ansi green)($platform)(ansi reset)"

    # System-level rebuild (darwin/nixos)
    match $platform {
        "darwin" => {
            print $"(ansi cyan)Running darwin-rebuild switch...(ansi reset)"
            darwin-rebuild switch --flake $flake
        }
        "nixos" => {
            print $"(ansi cyan)Running nixos-rebuild switch...(ansi reset)"
            sudo nixos-rebuild switch --flake $flake
        }
        _ => {
            print $"(ansi yellow)No system config for this platform(ansi reset)"
        }
    }

    # Home-manager
    let hm_config = get-hm-config
    print $"(ansi cyan)Running home-manager switch for: (ansi green)($hm_config)(ansi reset)"
    home-manager switch --flake $"($flake)#($hm_config)"
}

# Build configuration without applying
export def "hms build" [] {
    let platform = get-platform
    let flake = ($FLAKE_DIR | path expand)

    print $"(ansi cyan)Platform: (ansi green)($platform)(ansi reset)"

    match $platform {
        "darwin" => {
            print $"(ansi cyan)Building darwin config...(ansi reset)"
            darwin-rebuild build --flake $flake
        }
        "nixos" => {
            print $"(ansi cyan)Building nixos config...(ansi reset)"
            nixos-rebuild build --flake $flake
        }
        _ => {}
    }

    let hm_config = get-hm-config
    print $"(ansi cyan)Building home-manager config: (ansi green)($hm_config)(ansi reset)"
    home-manager build --flake $"($flake)#($hm_config)"
}

# Check flake validity
export def "hms check" [] {
    let flake = ($FLAKE_DIR | path expand)
    print $"(ansi cyan)Checking flake...(ansi reset)"
    nix flake check $flake
}

# Update flake inputs
export def "hms update" [
    input?: string  # Optional: specific input to update (e.g., "nixpkgs")
] {
    let flake = ($FLAKE_DIR | path expand)

    if ($input | is-empty) {
        print $"(ansi cyan)Updating all flake inputs...(ansi reset)"
        nix flake update $flake
    } else {
        print $"(ansi cyan)Updating ($input)...(ansi reset)"
        nix flake update $input --flake $flake
    }
}

# Garbage collect old generations
export def "hms gc" [
    --older-than: string = "7d"  # Delete generations older than this (default: 7d)
] {
    print $"(ansi cyan)Collecting garbage older than ($older_than)...(ansi reset)"

    # Nix store GC
    nix-collect-garbage --delete-older-than $older_than

    # Home-manager generations
    print $"(ansi cyan)Listing home-manager generations:(ansi reset)"
    home-manager generations
}

# Show diff between current and pending config
export def "hms diff" [] {
    let platform = get-platform
    let flake = ($FLAKE_DIR | path expand)
    let hm_config = get-hm-config

    print $"(ansi cyan)Building pending config...(ansi reset)"

    # Build to a result link
    home-manager build --flake $"($flake)#($hm_config)"

    # Compare with current
    if ("result" | path exists) {
        print $"(ansi cyan)Changes:(ansi reset)"
        nix store diff-closures ~/.local/state/nix/profiles/home-manager ./result
        rm -f result
    }
}

# Show current configuration info
export def "hms info" [] {
    let platform = get-platform
    let hm_config = get-hm-config
    let flake = ($FLAKE_DIR | path expand)

    print $"(ansi cyan)Platform:    (ansi green)($platform)(ansi reset)"
    print $"(ansi cyan)Hostname:    (ansi green)(get-hostname)(ansi reset)"
    print $"(ansi cyan)HM Config:   (ansi green)($hm_config)(ansi reset)"
    print $"(ansi cyan)Flake:       (ansi green)($flake)(ansi reset)"
    print ""

    print $"(ansi cyan)Available configurations:(ansi reset)"
    nix flake show $flake 2>/dev/null | lines | where { $in =~ "(darwin|nixos|home)" }
}

# Quick alias for switch (most common operation)
export def hms [] {
    hms switch
}
