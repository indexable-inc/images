# Function to update LS_COLORS based on macOS theme
def update_ls_colors [] {
    # # Check if vivid command is available
    # if (which vivid | is-empty) {
    #     # Fallback to a basic LS_COLORS if vivid isn't available
    #     $env.LS_COLORS = "di=34:ln=35:so=32:pi=33:ex=31:bd=34;46:cd=34;43:su=30;41:sg=30;46:tw=30;42:ow=30;43"
    #     return
    # }

    # # Redirect stderr to null to suppress the "does not exist" message
    # let dark_mode = (defaults read -g AppleInterfaceStyle 2>/dev/null | complete | get stdout | str trim)
    # if $dark_mode == "Dark" {
    #     $env.LS_COLORS = (vivid generate snazzy)
    # } else {
    #     $env.LS_COLORS = (vivid generate one-light)
    # }

    $env.LS_COLORS = (vivid generate one-light)
}