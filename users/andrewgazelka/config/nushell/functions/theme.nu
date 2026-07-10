# Theme-related functions for Nushell

# Detect system theme (light or dark)
export def get_system_theme [] {
    if ($nu.os-info.name == "macos") {
        # macOS: use defaults command
        let appearance = (defaults read -g AppleInterfaceStyle | complete)
        if $appearance.exit_code == 0 and ($appearance.stdout | str trim) == "Dark" {
            "dark"
        } else {
            "light"
        }
    } else {
        # Linux: default to dark (could check GNOME/KDE settings if needed)
        "dark"
    }
}

# Set up theme-based environment variables and LS_COLORS
export def setup_theme [] {
    let system_theme = (get_system_theme)
    $env.SYSTEM_THEME = $system_theme
    $env.IS_DARK_MODE = ($system_theme == "dark")
    $env.IS_LIGHT_MODE = ($system_theme == "light")

    # Set colors based on system theme
    if $env.IS_DARK_MODE {
        $env.LS_COLORS = (vivid generate snazzy)
    } else {
        $env.LS_COLORS = (vivid generate one-light)
    }
}
