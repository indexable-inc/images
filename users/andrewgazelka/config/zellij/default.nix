# Zellij configuration and plugin wiring. Nix owns the generated KDL.
{
  configRoot,
  stdenvNoCC,
  xdgConfigHome,
  zellijPlugins,
}: let
  minimalTabBar = stdenvNoCC.mkDerivation {
    pname = "minimal-tab-bar";
    version = "vendored";
    src = configRoot + "/zellij/plugins/minimal-tab-bar.wasm";
    dontUnpack = true;
    doCheck = true;
    checkPhase = ''
      # shell
      runHook preCheck
      test "$(od -An -tx1 -N4 "$src" | tr -d ' ')" = "0061736d"
      runHook postCheck
    '';
    installPhase = ''
      # shell
      runHook preInstall
      cp "$src" "$out"
      runHook postInstall
    '';
  };
in {
  extraConfig = ''
    // Zellij Configuration - Modern setup compatible with AeroSpace
    // AeroSpace uses: Alt-hjkl (focus), Alt-Shift-hjkl (join), Ctrl-Alt-Shift-hjkl (move)
    // Zellij uses: Ctrl-hjkl (pane focus), Ctrl-Shift (pane management)

    // Minimal dark theme - no color alternation
    themes {
        minimal {
            ribbon_selected {
                base 30 30 30           // Dark text on light bg
                background 160 160 160  // Light gray active tab
                emphasis_0 160 160 160
                emphasis_1 160 160 160
                emphasis_2 160 160 160
                emphasis_3 160 160 160
            }
            ribbon_unselected {
                base 120 120 120        // Gray text
                background 45 45 45     // Dark bg - same for all unselected
                emphasis_0 45 45 45
                emphasis_1 45 45 45     // SAME as background = no alternation
                emphasis_2 45 45 45
                emphasis_3 45 45 45
            }
            text_unselected {
                base 160 160 160
                background 26 26 26
                emphasis_0 120 120 120
                emphasis_1 120 120 120
                emphasis_2 120 120 120
                emphasis_3 120 120 120
            }
            text_selected {
                base 200 200 200
                background 40 40 40
                emphasis_0 140 140 140
                emphasis_1 140 140 140
                emphasis_2 140 140 140
                emphasis_3 140 140 140
            }
            frame_selected {
                base 100 100 100
                background 0
                emphasis_0 80 80 80
                emphasis_1 80 80 80
                emphasis_2 80 80 80
                emphasis_3 80 80 80
            }
            frame_highlight {
                base 140 140 140
                background 0
                emphasis_0 100 100 100
                emphasis_1 100 100 100
                emphasis_2 100 100 100
                emphasis_3 100 100 100
            }
            exit_code_success {
                base 120 160 120
                background 0
                emphasis_0 100 100 100
                emphasis_1 100 100 100
                emphasis_2 100 100 100
                emphasis_3 100 100 100
            }
            exit_code_error {
                base 180 100 100
                background 0
                emphasis_0 100 100 100
                emphasis_1 100 100 100
                emphasis_2 100 100 100
                emphasis_3 100 100 100
            }
            table_title {
                base 140 140 140
                background 0
                emphasis_0 100 100 100
                emphasis_1 100 100 100
                emphasis_2 100 100 100
                emphasis_3 100 100 100
            }
            table_cell_selected {
                base 200 200 200
                background 50 50 50
                emphasis_0 140 140 140
                emphasis_1 140 140 140
                emphasis_2 140 140 140
                emphasis_3 140 140 140
            }
            table_cell_unselected {
                base 160 160 160
                background 30 30 30
                emphasis_0 120 120 120
                emphasis_1 120 120 120
                emphasis_2 120 120 120
                emphasis_3 120 120 120
            }
            list_selected {
                base 200 200 200
                background 50 50 50
                emphasis_0 140 140 140
                emphasis_1 140 140 140
                emphasis_2 140 140 140
                emphasis_3 140 140 140
            }
            list_unselected {
                base 160 160 160
                background 30 30 30
                emphasis_0 120 120 120
                emphasis_1 120 120 120
                emphasis_2 120 120 120
                emphasis_3 120 120 120
            }
            multiplayer_user_colors {
                player_1 150 150 150
                player_2 130 130 130
                player_3 110 110 110
                player_4 100 100 100
                player_5 90 90 90
                player_6 80 80 80
                player_7 70 70 70
                player_8 60 60 60
                player_9 50 50 50
                player_10 40 40 40
            }
        }
    }
    theme "minimal"

    // Minimal UI - no pane frames, clean look
    ui {
        pane_frames {
            rounded_corners true
            hide_session_name true
        }
    }

    // Hide pane frames for cleaner look
    simplified_ui true
    pane_frames false

    // Default shell
    default_shell "nu"

    // Clipboard
    copy_on_select true
    copy_command "pbcopy"

    // Mouse support
    mouse_mode true

    // Force 24-bit color support
    env {
        TERM "xterm-256color"
        COLORTERM "truecolor"
    }

    // Session management
    session_serialization true
    serialize_pane_viewport true

    // Zellij's env block already supplies truecolor to the editor.
    scrollback_editor "nvim"
    scroll_buffer_size 50000

    // Disable startup tips
    show_startup_tips false

    // Layout - use built-in compact for now
    default_layout "compact"

    // Keybindings - using Ctrl-based to avoid AeroSpace conflicts
    keybinds clear-defaults=true {
        // Locked mode - Ctrl+g to enter, prevents zellij from capturing keys
        locked {
            bind "Ctrl g" { SwitchToMode "Normal"; }
        }

        // Normal mode - main navigation
        normal {
            // Lock zellij (pass-through to terminal)
            bind "Ctrl g" { SwitchToMode "Locked"; }

            // Pane focus with Ctrl-hjkl (no conflict with AeroSpace Alt-hjkl)
            bind "Ctrl h" { MoveFocus "Left"; }
            bind "Ctrl j" { MoveFocus "Down"; }
            bind "Ctrl k" { MoveFocus "Up"; }
            bind "Ctrl l" { MoveFocus "Right"; }

            // Tab navigation
            bind "Ctrl n" { GoToNextTab; }
            bind "Ctrl p" { GoToPreviousTab; }

            // Quick splits
            bind "Ctrl \\" { NewPane "Right"; }
            bind "Ctrl -" { NewPane "Down"; }

            // Close pane
            bind "Ctrl x" { CloseFocus; }

            // Toggle fullscreen
            bind "Ctrl z" { ToggleFocusFullscreen; }

            // Toggle floating
            bind "Ctrl f" { ToggleFloatingPanes; }

            // Enter modes
            bind "Ctrl Space" { SwitchToMode "Pane"; }
            bind "Ctrl t" { SwitchToMode "Tab"; }
            bind "Ctrl s" { SwitchToMode "Scroll"; }
            bind "Ctrl r" { SwitchToMode "Resize"; }
            bind "Ctrl o" { SwitchToMode "Session"; }
        }

        // Pane mode - Ctrl+Space then key
        pane {
            bind "Esc" "Ctrl c" { SwitchToMode "Normal"; }

            // Focus
            bind "h" "Left" { MoveFocus "Left"; SwitchToMode "Normal"; }
            bind "j" "Down" { MoveFocus "Down"; SwitchToMode "Normal"; }
            bind "k" "Up" { MoveFocus "Up"; SwitchToMode "Normal"; }
            bind "l" "Right" { MoveFocus "Right"; SwitchToMode "Normal"; }

            // New panes
            bind "n" { NewPane; SwitchToMode "Normal"; }
            bind "\\" "|" { NewPane "Right"; SwitchToMode "Normal"; }
            bind "-" "_" { NewPane "Down"; SwitchToMode "Normal"; }

            // Close
            bind "x" { CloseFocus; SwitchToMode "Normal"; }

            // Fullscreen
            bind "z" "f" { ToggleFocusFullscreen; SwitchToMode "Normal"; }

            // Floating
            bind "w" { ToggleFloatingPanes; SwitchToMode "Normal"; }
            bind "e" { TogglePaneEmbedOrFloating; SwitchToMode "Normal"; }

            // Rename
            bind "r" { SwitchToMode "RenamePane"; PaneNameInput 0; }
        }

        // Tab mode - Ctrl+t then key
        tab {
            bind "Esc" "Ctrl c" { SwitchToMode "Normal"; }

            // Navigation
            bind "h" "Left" { GoToPreviousTab; }
            bind "l" "Right" { GoToNextTab; }
            bind "j" "Down" { GoToNextTab; }
            bind "k" "Up" { GoToPreviousTab; }

            // Direct tab access
            bind "1" { GoToTab 1; SwitchToMode "Normal"; }
            bind "2" { GoToTab 2; SwitchToMode "Normal"; }
            bind "3" { GoToTab 3; SwitchToMode "Normal"; }
            bind "4" { GoToTab 4; SwitchToMode "Normal"; }
            bind "5" { GoToTab 5; SwitchToMode "Normal"; }
            bind "6" { GoToTab 6; SwitchToMode "Normal"; }
            bind "7" { GoToTab 7; SwitchToMode "Normal"; }
            bind "8" { GoToTab 8; SwitchToMode "Normal"; }
            bind "9" { GoToTab 9; SwitchToMode "Normal"; }

            // Create/close
            bind "n" { NewTab; SwitchToMode "Normal"; }
            bind "x" { CloseTab; SwitchToMode "Normal"; }

            // Rename
            bind "r" { SwitchToMode "RenameTab"; TabNameInput 0; }

            // Sync panes (type in all)
            bind "s" { ToggleActiveSyncTab; SwitchToMode "Normal"; }

            // Break pane to new tab
            bind "b" { BreakPane; SwitchToMode "Normal"; }
        }

        // Resize mode - Ctrl+r then hjkl
        resize {
            bind "Esc" "Ctrl c" { SwitchToMode "Normal"; }

            bind "h" "Left" { Resize "Increase Left"; }
            bind "j" "Down" { Resize "Increase Down"; }
            bind "k" "Up" { Resize "Increase Up"; }
            bind "l" "Right" { Resize "Increase Right"; }

            bind "H" { Resize "Decrease Left"; }
            bind "J" { Resize "Decrease Down"; }
            bind "K" { Resize "Decrease Up"; }
            bind "L" { Resize "Decrease Right"; }

            // Quick resize with +/-
            bind "=" "+" { Resize "Increase"; }
            bind "-" { Resize "Decrease"; }
        }

        // Scroll/search mode - Ctrl+s
        scroll {
            bind "Esc" "Ctrl c" { SwitchToMode "Normal"; }

            bind "j" "Down" { ScrollDown; }
            bind "k" "Up" { ScrollUp; }
            bind "d" "Ctrl d" { HalfPageScrollDown; }
            bind "u" "Ctrl u" { HalfPageScrollUp; }
            bind "f" "Ctrl f" { PageScrollDown; }
            bind "b" "Ctrl b" { PageScrollUp; }
            bind "g" { ScrollToTop; }
            bind "G" { ScrollToBottom; }

            // Search
            bind "/" { SwitchToMode "EnterSearch"; SearchInput 0; }
            bind "n" { Search "down"; }
            bind "N" { Search "up"; }

            // Edit scrollback in editor
            bind "e" { EditScrollback; SwitchToMode "Normal"; }
        }

        // Search mode
        search {
            bind "Esc" "Ctrl c" { SwitchToMode "Normal"; }

            bind "j" "Down" { ScrollDown; }
            bind "k" "Up" { ScrollUp; }
            bind "d" "Ctrl d" { HalfPageScrollDown; }
            bind "u" "Ctrl u" { HalfPageScrollUp; }

            bind "n" { Search "down"; }
            bind "N" { Search "up"; }

            bind "c" { SearchToggleOption "CaseSensitivity"; }
            bind "w" { SearchToggleOption "Wrap"; }
            bind "o" { SearchToggleOption "WholeWord"; }
        }

        entersearch {
            bind "Esc" "Ctrl c" { SwitchToMode "Scroll"; }
            bind "Enter" { SwitchToMode "Search"; }
        }

        // Session mode - Ctrl+o
        session {
            bind "Esc" "Ctrl c" { SwitchToMode "Normal"; }

            bind "d" { Detach; }
            bind "w" {
                LaunchOrFocusPlugin "session-manager" {
                    floating true
                    move_to_focused_tab true
                }
                SwitchToMode "Normal"
            }
        }

        // Rename modes
        renametab {
            bind "Esc" "Ctrl c" { UndoRenameTab; SwitchToMode "Tab"; }
            bind "Enter" { SwitchToMode "Normal"; }
        }

        renamepane {
            bind "Esc" "Ctrl c" { UndoRenamePane; SwitchToMode "Pane"; }
            bind "Enter" { SwitchToMode "Normal"; }
        }

        // Shared bindings across modes
        shared_except "locked" {
            // Quick quit
            bind "Ctrl q" { Quit; }
        }
    }

    // Plugins
    plugins {
        tab-bar location="zellij:tab-bar"
        status-bar location="zellij:status-bar"
        strider location="zellij:strider"
        compact-bar location="zellij:compact-bar" {
            tooltip "F1"  // Hints only show when F1 is pressed
        }
        session-manager location="zellij:session-manager"
        filepicker location="zellij:strider" {
            cwd "/"
        }
    }
  '';
  layouts.minimal = ''
    layout {
        default_tab_template {
            pane size=1 borderless=true {
                plugin location="file:${xdgConfigHome}/zellij/plugins/minimal-tab-bar.wasm"
            }
            children
        }
    }
  '';
  plugins = [
    minimalTabBar
    zellijPlugins.zjstatus
  ];
}
