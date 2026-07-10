# Textual application configuration. Nix is the canonical source.
{
  "btop-btop-conf" = ''
    #? Config file for btop v.1.4.7

    #* Name of a btop++/bpytop/bashtop formatted ".theme" file, "Default" and "TTY" for builtin themes.
    #* Themes should be placed in "../share/btop/themes" relative to binary or "$HOME/.config/btop/themes"
    color_theme = "tomorrow-night.theme"

    #* If the theme set background should be shown, set to False if you want terminal background transparency.
    theme_background = false

    #* Sets if 24-bit truecolor should be used, will convert 24-bit colors to 256 color (6x6x6 color cube) if false.
    truecolor = true

    #* Set to true to force tty mode regardless if a real tty has been detected or not.
    #* Will force 16-color mode and TTY theme, set all graph symbols to "tty" and swap out other non tty friendly symbols.
    force_tty = false

    #* Option to disable presets. Either the default preset, custom presets, or all presets.
    #* "Off" All presets are enabled.
    #* "Default" preset is disabled.#* "Custom" presets are disabled.#* "All" presets are disabled.
    disable_presets = "Off"

    #* Define presets for the layout of the boxes. Preset 0 is always all boxes shown with default settings. Max 9 presets.
    #* Format: "box_name:P:G,box_name:P:G" P=(0 or 1) for alternate positions, G=graph symbol to use for box.
    #* Use whitespace " " as separator between different presets.
    #* Example: "cpu:0:default,mem:0:tty,proc:1:default cpu:0:braille,proc:0:tty"
    presets = "cpu:1:default,proc:0:default cpu:0:default,mem:0:default,net:0:default cpu:0:block,net:0:tty"

    #* Set to True to enable "h,j,k,l,g,G" keys for directional control in lists.
    #* Conflicting keys for h:"help" and k:"kill" is accessible while holding shift.
    vim_keys = true

    #* Disable all mouse events.
    disable_mouse = false

    #* Rounded corners on boxes, is ignored if TTY mode is ON.
    rounded_corners = true

    #* Use terminal synchronized output sequences to reduce flickering on supported terminals.
    terminal_sync = true

    #* Default symbols to use for graph creation, "braille", "block" or "tty".
    #* "braille" offers the highest resolution but might not be included in all fonts.
    #* "block" has half the resolution of braille but uses more common characters.
    #* "tty" uses only 3 different symbols but will work with most fonts and should work in a real TTY.
    #* Note that "tty" only has half the horizontal resolution of the other two, so will show a shorter historical view.
    graph_symbol = "block"

    # Graph symbol to use for graphs in cpu box, "default", "braille", "block" or "tty".
    graph_symbol_cpu = "default"

    # Graph symbol to use for graphs in gpu box, "default", "braille", "block" or "tty".
    graph_symbol_gpu = "default"

    # Graph symbol to use for graphs in cpu box, "default", "braille", "block" or "tty".
    graph_symbol_mem = "default"

    # Graph symbol to use for graphs in cpu box, "default", "braille", "block" or "tty".
    graph_symbol_net = "default"

    # Graph symbol to use for graphs in cpu box, "default", "braille", "block" or "tty".
    graph_symbol_proc = "default"

    #* Manually set which boxes to show. Available values are "cpu mem net proc" and "gpu0" through "gpu5", separate values with whitespace.
    shown_boxes = "proc cpu"

    #* Update time in milliseconds, recommended 2000 ms or above for better sample times for graphs.
    update_ms = 500

    #* Processes sorting, "pid" "program" "arguments" "threads" "user" "memory" "io read" "io write" "io total" "cpu lazy" "cpu direct",
    #* "cpu lazy" sorts top process over time (easier to follow), "cpu direct" updates top process directly.
    proc_sorting = "threads"

    #* Reverse sorting order, True or False.
    proc_reversed = false

    #* Show processes as a tree.
    proc_tree = true

    #* Use the cpu graph colors in the process list.
    proc_colors = true

    #* Use a darkening gradient in the process list.
    proc_gradient = true

    #* If process cpu usage should be of the core it's running on or usage of the total available cpu power.
    proc_per_core = true

    #* Show process memory as bytes instead of percent.
    proc_mem_bytes = true

    #* Show cpu graph for each process.
    proc_cpu_graphs = true

    #* Use /proc/[pid]/smaps for memory information in the process info box (very slow but more accurate)
    proc_info_smaps = false

    #* Show proc box on left side of screen instead of right.
    proc_left = false

    #* (Linux) Filter processes tied to the Linux kernel(similar behavior to htop).
    proc_filter_kernel = false

    #* Should the process list follow the selected process when detailed view is open.
    proc_follow_detailed = true

    #* In tree-view, always accumulate child process resources in the parent process.
    proc_aggregate = true

    #* In tree-view, auto-collapse processes with this many or more direct children when
    #* entering tree mode. 0 to disable. Useful for collapsing multi-process apps like browsers.
    proc_tree_auto_collapse = 0

    #* Should cpu and memory usage display be preserved for dead processes when paused.
    keep_dead_proc_usage = false

    #* Sets the CPU stat shown in upper half of the CPU graph, "total" is always available.
    #* Select from a list of detected attributes from the options menu.
    cpu_graph_upper = "total"

    #* Sets the CPU stat shown in lower half of the CPU graph, "total" is always available.
    #* Select from a list of detected attributes from the options menu.
    cpu_graph_lower = "total"

    #* If gpu info should be shown in the cpu box. Available values = "Auto", "On" and "Off".
    show_gpu_info = "Auto"

    #* Toggles if the lower CPU graph should be inverted.
    cpu_invert_lower = true

    #* Set to True to completely disable the lower CPU graph.
    cpu_single_graph = false

    #* Show cpu box at bottom of screen instead of top.
    cpu_bottom = false

    #* Shows the system uptime in the CPU box.
    show_uptime = true

    #* Shows the CPU package current power consumption in watts. Requires running `make setcap` or `make setuid` or running with sudo.
    show_cpu_watts = true

    #* Show cpu temperature.
    check_temp = true

    #* Which sensor to use for cpu temperature, use options menu to select from list of available sensors.
    cpu_sensor = "Auto"

    #* Show temperatures for cpu cores also if check_temp is True and sensors has been found.
    show_coretemp = true

    #* Set a custom mapping between core and coretemp, can be needed on certain cpus to get correct temperature for correct core.
    #* Use lm-sensors or similar to see which cores are reporting temperatures on your machine.
    #* Format "x:y" x=core with wrong temp, y=core with correct temp, use space as separator between multiple entries.
    #* Example: "4:0 5:1 6:3"
    cpu_core_map = ""

    #* Which temperature scale to use, available values: "celsius", "fahrenheit", "kelvin" and "rankine".
    temp_scale = "fahrenheit"

    #* Use base 10 for bits/bytes sizes, KB = 1000 instead of KiB = 1024.
    base_10_sizes = false

    #* Show CPU frequency.
    show_cpu_freq = true

    #* Draw a clock at top of screen, formatting according to strftime, empty string to disable.
    #* Special formatting: /host = hostname | /user = username | /uptime = system uptime
    clock_format = "%X"

    #* Update main ui in background when menus are showing, set this to false if the menus is flickering too much for comfort.
    background_update = true

    #* Custom cpu model name, empty string to disable.
    custom_cpu_name = ""

    #* Optional filter for shown disks, should be full path of a mountpoint, separate multiple values with whitespace " ".
    #* Only disks matching the filter will be shown. Prepend exclude= to only show disks not matching the filter. Examples: disk_filter="/boot /home/user", disks_filter="exclude=/boot /home/user"
    disks_filter = "/"

    #* Show graphs instead of meters for memory values.
    mem_graphs = true

    #* Show mem box below net box instead of above.
    mem_below_net = false

    #* Count ZFS ARC in cached and available memory.
    zfs_arc_cached = true

    #* If swap memory should be shown in memory box.
    show_swap = true

    #* Show swap as a disk, ignores show_swap value above, inserts itself after first disk.
    swap_disk = true

    #* If mem box should be split to also show disks info.
    show_disks = false

    #* Filter out non physical disks. Set this to False to include network disks, RAM disks and similar.
    only_physical = true

    #* Read disks list from /etc/fstab. This also disables only_physical.
    use_fstab = true

    #* Setting this to True will hide all datasets, and only show ZFS pools. (IO stats will be calculated per-pool)
    zfs_hide_datasets = false

    #* Set to true to show available disk space for privileged users.
    disk_free_priv = false

    #* Toggles if io activity % (disk busy time) should be shown in regular disk usage view.
    show_io_stat = true

    #* Toggles io mode for disks, showing big graphs for disk read/write speeds.
    io_mode = false

    #* Set to True to show combined read/write io graphs in io mode.
    io_graph_combined = false

    #* Set the top speed for the io graphs in MiB/s (100 by default), use format "mountpoint:speed" separate disks with whitespace " ".
    #* Example: "/mnt/media:100 /:20 /boot:1".
    io_graph_speeds = ""

    #* Swap the positions of the upload and download speed graphs. When true, upload will be on top.
    swap_upload_download = false

    #* Set fixed values for network graphs in Mebibits. Is only used if net_auto is also set to False.
    net_download = 100

    net_upload = 100

    #* Use network graphs auto rescaling mode, ignores any values set above and rescales down to 10 Kibibytes at the lowest.
    net_auto = false

    #* Sync the auto scaling for download and upload to whichever currently has the highest scale.
    net_sync = true

    #* Starts with the Network Interface specified here.
    net_iface = ""

    #* "True" shows bitrates in base 10 (Kbps, Mbps). "False" shows bitrates in binary sizes (Kibps, Mibps, etc.). "Auto" uses base_10_sizes.
    base_10_bitrate = "Auto"

    #* Show battery stats in top right if battery is present.
    show_battery = true

    #* Which battery to use if multiple are present. "Auto" for auto detection.
    selected_battery = "Auto"

    #* Show power stats of battery next to charge indicator.
    show_battery_watts = true

    #* Set loglevel for "~/.local/state/btop.log" levels are: "ERROR" "WARNING" "INFO" "DEBUG".
    #* The level set includes all lower levels, i.e. "DEBUG" will show all logging info.
    log_level = "WARNING"

    #* Automatically save current settings to config file on exit.
    save_config_on_exit = true

    #* Measure PCIe throughput on NVIDIA cards, may impact performance on certain cards.
    nvml_measure_pcie_speeds = true

    #* Measure PCIe throughput on AMD cards, may impact performance on certain cards.
    rsmi_measure_pcie_speeds = true

    #* Horizontally mirror the GPU graph.
    gpu_mirror_graph = true

    #* Set which GPU vendors to show. Available values are "nvidia amd intel apple"
    shown_gpus = "nvidia amd intel apple"

    #* Custom gpu0 model name, empty string to disable.
    custom_gpu_name0 = ""

    #* Custom gpu1 model name, empty string to disable.
    custom_gpu_name1 = ""

    #* Custom gpu2 model name, empty string to disable.
    custom_gpu_name2 = ""

    #* Custom gpu3 model name, empty string to disable.
    custom_gpu_name3 = ""

    #* Custom gpu4 model name, empty string to disable.
    custom_gpu_name4 = ""

    #* Custom gpu5 model name, empty string to disable.
    custom_gpu_name5 = ""
  '';
  "kitty-kitty-conf" = ''
    # Font configuration
    font_family FiraCode Nerd Font
    font_size 10
    bold_font FiraCode Nerd Font Bold
    italic_font FiraCode Nerd Font Italic
    bold_italic_font FiraCode Nerd Font Bold Italic

    # Enable ligatures
    disable_ligatures never

    # Font features for better ligature support
    font_features FiraCode-Retina +liga +calt +ss01 +ss02 +ss03 +ss04 +ss05 +ss06 +ss07 +ss08 +ss09 +ss10

    # Line height and letter spacing
    adjust_line_height 0
    adjust_column_width 0

    # Cursor configuration
    cursor_shape block
    shell_integration no_cursor

    # Scrollback
    scrollback_lines -1

    # Include theme
    # You can manually download and include a theme here:
    # include ~/.config/kitty/themes/PaperColor_Light.conf
  '';
  "ideavimrc" = ''
    set clipboard+=unnamed
    let mapleader=" "

    " FunctionTextObj
    " dif  - delete the function body
    " caf  - change the entire function
    " vif  - visually select the function body
    " yaf  - yank (copy) the entire function
    set functiontextobj



    Plug 'dbakker/vim-paragraph-motion'



    " <count>ai	An Indentation level and line above.
    " <count>ii	Inner Indentation level (no line above).
    " <count>aI	An Indentation level and lines above/below.
    " <count>iI	Inner Indentation level (no lines above/below).
    Plug 'michaeljsmith/vim-indent-object'

    Plug 'preservim/nerdtree'
    Plug 'terryma/vim-multiple-cursors'

    " cx
    " On the first use, define the first {motion} to exchange. On the second use, define the second {motion} and perform the exchange.
    "
    " cxx
    "
    " Like cx, but use the current line.
    Plug 'tommcdo/vim-exchange'

    Plug 'tpope/vim-commentary'
    Plug 'tpope/vim-surround'
    Plug 'vim-scripts/argtextobj.vim'


    " Remap multiple-cursors shortcuts to match terryma/vim-multiple-cursors
    nmap <C-n> <Plug>NextWholeOccurrence
    xmap <C-n> <Plug>NextWholeOccurrence
    "nmap <C-n> <Plug>NextOccurrence
    "xmap <C-n> <Plug>NextOccurrence
    xmap <C-x> <Plug>SkipOccurrence
    xmap <C-p> <Plug>RemoveOccurrence

    " Note that the default <A-n> and g<A-n> shortcuts don't work on Mac due to dead keys.
    " <A-n> is used to enter accented text e.g. ñ
    " Feel free to pick your own mappings that are not affected. I like to use <leader>
    nmap <leader><C-n> <Plug>AllWholeOccurrences
    xmap <leader><C-n> <Plug>AllWholeOccurrences
    nmap <leader>g<C-n> <Plug>AllOccurrences
    xmap <leader>g<C-n> <Plug>AllOccurrences

    set rnu
    set scrolloff=30
    "set scrolloff=200

    set hlsearch

    set nowrapscan

    nmap <S-A-j> <Action>(MethodDown)
    nmap <S-A-k> <Action>(MethodUp)



    nmap <leader>j <Action>(NextOccurence)
    nmap <leader>k <Action>(PreviousOccurence)

    nmap <leader>q <Action>(Rerun)

    " Basic jumps
    nmap <leader><leader>s :action KJumpAction<cr>
    nmap <leader><leader>w :action KJumpAction.Word0<cr>
    nmap <leader><leader>l :action KJumpAction.Line<cr>

    nmap f :action KJumpAction.Word0<cr>
    nmap F :action KJumpAction.Line<cr>

    " Additional jumps
    nmap <leader><leader>c :action KJumpAction.Char2<cr>
    nmap <leader><leader>f :action KJumpAction.Word1<cr>


    let g:argtextobj_pairs="(:),{:},<:>,{:}"
    nmap <leader>q <Action>(Run)


    nmap <leader><leader>h <Action>(Vcs.ShowTabbedFileHistory)
    nmap <leader>o ysiw>iOption<Esc>




    nnoremap ma mA
    nnoremap mb mB
    nnoremap mc mC
    nnoremap md mD
    nnoremap me mE
    nnoremap mf mF
    nnoremap mg mG
    nnoremap mh mH
    nnoremap mi mI
    nnoremap mj mJ
    nnoremap mk mK
    nnoremap ml mL
    nnoremap mm mM
    nnoremap mn mN
    nnoremap mo mO
    nnoremap mp mP
    nnoremap mq mQ
    nnoremap mr mR
    nnoremap ms mS
    nnoremap mt mT
    nnoremap mu mU
    nnoremap mv mV
    nnoremap mw mW
    nnoremap mx mX
    nnoremap my mY
    nnoremap mz mZ

    nnoremap 'a 'Azt
    nnoremap 'b 'Bzt
    nnoremap 'c 'Czt
    nnoremap 'd 'Dzt
    nnoremap 'e 'Ezt
    nnoremap 'f 'Fzt
    nnoremap 'g 'Gzt
    nnoremap 'h 'Hzt
    nnoremap 'i 'Izt
    nnoremap 'j 'Jzt
    nnoremap 'k 'Kzt
    nnoremap 'l 'Lzt
    nnoremap 'm 'Mzt
    nnoremap 'n 'Nzt
    nnoremap 'o 'Ozt
    nnoremap 'p 'Pzt
    nnoremap 'q 'Qzt
    nnoremap 'r 'Rzt
    nnoremap 's 'Szt
    nnoremap 't 'Tzt
    nnoremap 'u 'Uzt
    nnoremap 'v 'Vzt
    nnoremap 'w 'Wzt
    nnoremap 'x 'Xzt
    nnoremap 'y 'Yzt
    nnoremap 'z 'Zzt
    set ideajoin
  '';
  "lldbinit" = ''
    # LLDB aliases
    command alias tl thread list
  '';
  "gitignore_global" = ''
    # Rust
    #
    .claude/settings.local.json
    .codex-output/
    .codex-reports/
    target-clippy
    node_modules
    target-check
    **/target-clippy
    **/_build
    dump*.json
    # IDE
    .idea
    # Xcode user data
    *.xcuserstate
    xcuserdata/
    # macOS
    .DS_Store
    # direnv
    .direnv
    # CMake
    CMakeCache.txt
    CMakeFiles/
    cmake_install.cmake
    Makefile
    *.cmake
    !CMakeLists.txt
    !*.cmake
    _deps/
    cpm-package-lock.cmake
    build/
    .zsync
    buck-out
    .wrangler
    **/.claude/settings.local.json
    **/.claude/worktrees/
    result
    target
    .vercel
    .sgrep
    # Python
    .pytest_cache/
    .ruff_cache/
    .worktrees/
    .Codex/
  '';
  "gitattributes" = ''
    # AST-aware merge for supported languages
    *.rs merge=ast-merge
    *.ts merge=ast-merge
    *.tsx merge=ast-merge
    *.mts merge=ast-merge
    *.cts merge=ast-merge
    *.js merge=ast-merge
    *.jsx merge=ast-merge
    *.mjs merge=ast-merge
    *.cjs merge=ast-merge
    *.py merge=ast-merge
    *.pyi merge=ast-merge
    *.go merge=ast-merge
    *.json merge=ast-merge
    *.jsonc merge=ast-merge
    *.toml merge=ast-merge
    *.yaml merge=ast-merge
    *.yml merge=ast-merge

    # Disabled: mergiraf as fallback for everything else
    # * merge=mergiraf
  '';
  "tmux--tmux-conf" = ''
    # CORE SETTINGS
    set -g default-terminal "tmux-256color"
    # Extended keys (CSI u encoding) for modern terminals like Ghostty
    set -s extended-keys on
    set -as terminal-features 'xterm-ghostty:extkeys'
    # Modern terminal feature support - all capabilities for xterm-256color terminals
    set -as terminal-features ',xterm-256color:RGB:256:clipboard:ccolour:cstyle:extkeys:focus:hyperlinks:margins:overline:rectfill:strikethrough:sync:title:usstyle'
    # Full Ghostty terminal capabilities
    set -as terminal-features ',xterm-ghostty:RGB:256:clipboard:ccolour:cstyle:extkeys:focus:hyperlinks:margins:overline:rectfill:sixel:strikethrough:sync:title:usstyle'
    # Full Kitty terminal capabilities
    set -as terminal-features ',xterm-kitty:RGB:256:clipboard:ccolour:cstyle:extkeys:focus:hyperlinks:margins:overline:rectfill:strikethrough:sync:title:usstyle'
    # Enable synchronized updates to prevent flickering
    set -g alternate-screen on
    set -g allow-passthrough on
    set -g mouse on                                    # Mouse support that actually works
    set -g base-index 1                                # Start windows at 1, not 0
    set -g pane-base-index 1                          # Start panes at 1
    set -g renumber-windows on                        # Renumber windows on close
    set -g automatic-rename off                       # Don't auto-rename based on command
    set -g allow-rename on                            # Allow programs to set title via escape sequences
    set -g set-titles on                              # Pass title to terminal emulator (Ghostty)
    set -g escape-time 0                               # No delay for escape key
    set -g default-shell "/bin/zsh"                    # Use zsh as default shell
    set -g default-command "/bin/zsh"                  # Use zsh for new panes/windows
    bind c new-window -c "#{pane_current_path}"       # New window in current dir
    bind '"' split-window -c "#{pane_current_path}"   # Horizontal split in current dir
    bind % split-window -h -c "#{pane_current_path}"  # Vertical split in current dir
    set -g history-limit 50000                         # Massive history
    set -g display-time 4000                          # Status messages for 4s
    set -g focus-events on                            # Focus events for vim
    set -g aggressive-resize on                       # Resize window to smallest client

    # ═══════════════════════════════════════════════════════════════════════════════
    # ACTIVITY MONITORING - Get notified when windows have activity
    # ═══════════════════════════════════════════════════════════════════════════════

    # Enable activity monitoring on all windows by default
    setw -g monitor-activity on                       # Monitor for any output activity
    setw -g monitor-bell on                           # Monitor for bell characters

    # Visual indicators in status bar (no terminal bell spam)
    set -g visual-activity off                        # Don't show message, just highlight
    set -g visual-bell off                            # Don't flash terminal
    set -g visual-silence off                         # Don't show silence message

    # Activity actions - 'other' means only alert for non-current windows
    set -g activity-action other                      # Alert on activity in other windows
    set -g bell-action other                          # Alert on bell in other windows
    set -g silence-action none                        # Disabled - using bell instead

    # ═══════════════════════════════════════════════════════════════════════════════
    # KEYBINDINGS
    # ═══════════════════════════════════════════════════════════════════════════════

    bind r source-file ~/.tmux.conf \; display-message "Config reloaded!"
    bind e run-shell "tmux capture-pane -pS - > /tmp/tmux-buffer && nvim '+ normal G' /tmp/tmux-buffer"
    bind -n M-e run-shell "tmux capture-pane -pS - > /tmp/tmux-buffer && nvim '+ normal G' /tmp/tmux-buffer"
    bind -n M-k clear-history \; display-message "History cleared!"

    # Toggle activity monitoring on current window
    bind m setw monitor-activity \; display-message "Activity monitoring: #{?monitor-activity,ON,OFF}"

    # Toggle silence monitoring (useful for Claude Code / long-running commands)
    bind M setw monitor-silence 5 \; display-message "Silence monitoring: 5s"
    bind -n M-M setw monitor-silence 0 \; display-message "Silence monitoring: OFF"

    # Let tilish handle all the keybindings - it provides i3-style defaults

    # ═══════════════════════════════════════════════════════════════════════════════
    # VIM-LIKE COPY MODE
    # ═══════════════════════════════════════════════════════════════════════════════

    setw -g mode-keys vi                                # Use vim keybindings in copy mode
    set -g set-clipboard on                             # Enable clipboard integration
    bind -T copy-mode-vi v send-keys -X begin-selection
    bind -T copy-mode-vi y send-keys -X copy-pipe-and-cancel "pbcopy"
    bind -T copy-mode-vi C-v send-keys -X rectangle-toggle
    bind -T copy-mode-vi Escape send-keys -X cancel

    # ═══════════════════════════════════════════════════════════════════════════════
    # GRAYSCALE THEME WITH ACTIVITY INDICATORS
    # ═══════════════════════════════════════════════════════════════════════════════

    set -g status-position bottom
    set -g status-style 'bg=#1c1c1c fg=#d0d0d0'
    set -g status-left-length 40
    set -g status-right-length 60
    set -g status-justify left
    set -g status-interval 1

    # Left: prefix indicator only
    set -g status-left '#{?client_prefix,#[fg=colour234 bg=colour108] PREFIX ,}'

    # Right: pane info + sync indicator
    set -g status-right '#[fg=#5f5f5f]#{?synchronize-panes,#[fg=#d75f5f,bold] SYNC ,}#[fg=#808080]#{pane_width}x#{pane_height} #[fg=#a8a8a8]#{cursor_x}:#{cursor_y} '

    # Window status format with bell indicator (bell = red text)
    # Active window: bright and bold with boxed number
    set -g window-status-current-format '#{?window_bell_flag,#[fg=#1c1c1c bg=#d46a6a],#[fg=#1c1c1c bg=#e0e0e0]}#[bold] #I #[bg=default] #{?window_bell_flag,#[fg=#e57373],#[fg=#ffffff]}#{b:pane_current_path} #W#{?window_zoomed_flag,#[fg=#d4a656] 󰊓,}'

    # Inactive windows: dimmed with boxed number (bell = red)
    set -g window-status-format '#{?window_bell_flag,#[fg=#1c1c1c bg=#d46a6a],#[fg=#c0c0c0 bg=#333333]} #I #[bg=default] #{?window_bell_flag,#[fg=#e57373],#[fg=#888888]}#{b:pane_current_path} #W'

    set -g window-status-separator '  '

    # Bell style for the window-status
    set -g window-status-bell-style 'fg=colour167,bold'        # Red bold for bell

    # ═══════════════════════════════════════════════════════════════════════════════
    # GRAYSCALE PANE BORDERS
    # ═══════════════════════════════════════════════════════════════════════════════

    set -g pane-border-style 'fg=#3a3a3a'
    set -g pane-active-border-style 'fg=#c0c0c0'
    set -g pane-border-status off

    # ═══════════════════════════════════════════════════════════════════════════════
    # MESSAGE AND MODE STYLES
    # ═══════════════════════════════════════════════════════════════════════════════

    set -g message-style 'fg=#e4e4e4,bg=#303030,bold'
    set -g message-command-style 'fg=#e4e4e4,bg=#444444'
    set -g mode-style 'fg=#1c1c1c,bg=#87af87,bold'            # Selection highlight in copy mode

    # ═══════════════════════════════════════════════════════════════════════════════
    # TMUX PLUGIN MANAGER (TPM)
    # ═══════════════════════════════════════════════════════════════════════════════

    set -g @plugin 'tmux-plugins/tpm'
    set -g @plugin 'jabirali/tmux-tilish'
    set -g @plugin 'nhdaly/tmux-better-mouse-mode'
    set -g @scroll-speed-num-lines-per-scroll 3    # lines per scroll tick (default 3, use decimal < 1 for slower)

    # tmux-notify: Desktop notifications when long commands finish
    # Prefix + m: Start monitoring, notify when finished
    # Prefix + M: Cancel monitoring (we override M above for silence, so use prefix + alt-m)
    set -g @plugin 'rickstaa/tmux-notify'
    set -g @tnotify-verbose 'on'
    set -g @tnotify-sleep-duration '5'

    # tmux-prefix-highlight: Show when prefix is active (integrated in status-left above)
    # We handle this manually in status-left for cleaner integration

    # TILISH CONFIGURATION - i3-style window management
    set -g @tilish-default 'main-vertical'      # Default layout like i3
    set -g @tilish-navigator 'on'                # Enable vim-like navigation
    # Tilish provides these keybindings:
    # Alt+Enter: New pane with intelligent split direction (like i3)
    # Alt+s: Toggle split direction (horizontal/vertical)
    # Alt+Shift+HJKL: Move panes around

    # Cmd+Enter for new pane (via Ghostty sending \x1b[13;9u)
    set -s user-keys[0] "\e[13;9u"
    bind -n User0 split-window -c "#{pane_current_path}"

    # Initialize TPM (keep this at the very bottom)
    run '~/.tmux/plugins/tpm/tpm'

    # Unbind Alt+Enter (tilish new pane) - prefer explicit splits
    unbind -n M-Enter
  '';
  "tracy-tracy-ini" = ''
    [core]
    threadedRendering = 1
    focusLostLimit = 1

    [timeline]
    targetFps = 60
    drawFrameTargets = 0
    dynamicColors = 1
    forceColors = 0
    ghostZones = 1
    shortenName = 4
    drawSamples = 1
    drawContextSwitches = 1
    plotHeight = 100
    horizontalScrollMultiplier = 1.000000
    verticalScrollMultiplier = 1.000000

    [memory]
    limit = 0
    percent = 80

    [achievements]
    enabled = 1
    asked = 1

    [ui]
    saveUserScale = 1
    userScale = 1.0

    [llm]
    enabled = 1
    address = http://localhost:11434
    model =
    embeddings =
    useragent = Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36
    searchIdentifier =
    searchApiKey =
  '';
  "cursor-keybindings-json" = ''
    [
      {
        "command": "-workbench.action.terminal.openNativeConsole",
        "key": "shift+cmd+c"
      },
      {
        "key": "ctrl+n",
        "command": "-editor.emmet.action.nextEditPoint"
      },
      {
        "key": "ctrl+n",
        "command": "-extension.changeCase.next"
      },
      {
        "command": "editor.action.addSelectionToPreviousFindMatch",
        "key": "alt+shift+n",
        "when": "editorFocus"
      },
      {
        "command": "editor.action.moveSelectionToNextFindMatch",
        "key": "alt+d",
        "when": "editorFocus"
      },
      {
        "command": "gitlens.copyRemoteFileUrlToClipboard",
        "key": "shift+cmd+c",
        "when": "editorTextFocus"
      },
      {
        "key": "ctrl+r",
        "command": "redo",
        "when": "editorTextFocus"
      },
      {
        "key": "cmd+b",
        "command": "-workbench.action.toggleSidebarVisibility"
      },
      {
        "key": "cmd+b",
        "command": "-mdx.toggleStrong",
        "when": "editorTextFocus && !editorReadonly && editorLangId == 'mdx'"
      },
      {
        "key": "ctrl+cmd+p",
        "command": "composerMode.plan"
      },
      {
        "key": "cmd+e",
        "command": "-composer.showBackgroundAgentHistory",
        "when": "backgroundComposerEnabled || showBackgroundAgentHistoryAction"
      },
      {
        "key": "shift+cmd+s",
        "command": "-workbench.action.files.saveAs"
      },
      {
        "key": "shift+cmd+s",
        "command": "-workbench.action.backgroundComposer.toggleSidebar",
        "when": "backgroundComposerEnabled"
      },
      {
        "key": "shift+cmd+s",
        "command": "-workbench.action.files.saveLocalFile",
        "when": "remoteFileDialogVisible"
      },
      {
        "key": "shift+cmd+s",
        "command": "workbench.action.gotoSymbol",
        "when": "editorTextFocus"
      },
      {
        "key": "cmd+]",
        "command": "-editor.action.indentLines",
        "when": "editorTextFocus && !editorReadonly"
      },
      {
        "key": "ctrl+d",
        "command": "-workbench.action.debug.run",
        "when": "!inDebugMode && !terminalFocus"
      },
      {
        "key": "ctrl+d",
        "command": "-workbench.action.debug.run",
        "when": "debuggersAvailable && !inDebugMode && !terminalFocus"
      },
      {
        "key": "ctrl+d",
        "command": "-deleteRight",
        "when": "textInputFocus"
      },
      {
        "key": "shift+cmd+b",
        "command": "editor.action.goToTypeDefinition",
        "when": "editorHasTypeDefinitionProvider && editorTextFocus"
      },
      {
        "key": "alt+cmd+k",
        "command": "-git.pushTo",
        "when": "!inDebugMode && !operationInProgress && !terminalFocus"
      },
      {
        "key": "alt+cmd+k",
        "command": "-keybindings.editor.recordSearchKeys",
        "when": "inKeybindings && inKeybindingsSearch"
      },
      {
        "key": "ctrl+j",
        "command": "editor.action.showHover",
        "when": "editorTextFocus"
      },
      {
        "key": "ctrl+n",
        "command": "selectNextSuggestion",
        "when": "suggestWidgetMultipleSuggestions && suggestWidgetVisible && textInputFocus"
      },
      {
        "key": "cmd+k",
        "command": "-git.commitAll",
        "when": "!inDebugMode && !operationInProgress && !terminalFocus"
      },
      {
        "key": "cmd+k cmd+r",
        "command": "-git.revertSelectedRanges",
        "when": "editorTextFocus && !operationInProgress && resourceScheme == 'file'"
      },
      {
        "key": "cmd+k alt+cmd+s",
        "command": "-git.stageSelectedRanges",
        "when": "editorTextFocus && !operationInProgress && resourceScheme == 'file'"
      },
      {
        "key": "cmd+k cmd+n",
        "command": "-git.unstageSelectedRanges",
        "when": "editorTextFocus && isInDiffEditor && isInDiffRightEditor && !operationInProgress && resourceScheme == 'git'"
      },
      {
        "key": "cmd+k v",
        "command": "-markdown.showPreviewToSide",
        "when": "!notebookEditorFocused && editorLangId == 'markdown'"
      },
      {
        "key": "cmd+k m",
        "command": "-notebook.cell.changeLanguage",
        "when": "notebookCellEditable && notebookEditable && notebookEditorFocused"
      },
      {
        "key": "cmd+k",
        "command": "-cursorai.action.generateInTerminal",
        "when": "terminalFocus && terminalHasBeenCreated || terminalFocus && terminalProcessSupported || terminalHasBeenCreated && terminalPromptBarVisible || terminalProcessSupported && terminalPromptBarVisible"
      },
      {
        "key": "cmd+k v",
        "command": "-typst-preview.preview",
        "when": "editorLangId == 'typst'"
      },
      {
        "key": "cmd+k",
        "command": "-editor.action.inlineDiffs.focusEditor",
        "when": "editorHasPromptBar && editorPromptBarFocused"
      },
      {
        "key": "cmd+k cmd+i",
        "command": "-workbench.action.showHover",
        "when": "!editorTextFocus"
      },
      {
        "key": "cmd+k",
        "command": "-workbench.debug.panel.action.clearReplAction",
        "when": "focusedView == 'workbench.panel.repl.view'"
      },
      // {
      //   "key": "cmd+k",
      //   "command": "-aipopup.action.modal.generate",
      //   "when": "editorFocus && !composerBarIsVisible"
      // },
      {
        "key": "ctrl+n",
        "command": "selectNextCodeAction",
        "when": "codeActionMenuVisible"
      },
      {
        "key": "ctrl+p",
        "command": "selectPrevCodeAction",
        "when": "codeActionMenuVisible"
      },
      {
        "key": "alt+j",
        "command": "editor.action.marker.nextInFiles",
        "when": "editorFocus"
      },
      {
        "key": "alt+k",
        "command": "editor.action.marker.prevInFiles",
        "when": "editorFocus"
      },
      {
        "key": "alt+j",
        "command": "list.focusDown",
        "when": "problemsViewFocus"
      },
      {
        "key": "alt+k",
        "command": "list.focusUp",
        "when": "problemsViewFocus"
      },
      {
        "key": "shift+enter",
        "command": "workbench.action.terminal.sendSequence",
        "args": {
          "text": "\\\r\n"
        },
        "when": "terminalFocus"
      },
      {
        "key": "ctrl+n",
        "command": "-workbench.action.quickOpenSelectNext",
        "when": "inQuickOpen"
      },
      {
        "key": "shift+cmd+[",
        "command": "-workbench.action.debug.prevConsole",
        "when": "inDebugRepl"
      },
      {
        "key": "shift+cmd+[",
        "command": "-workbench.action.terminal.focusPrevious",
        "when": "terminalHasBeenCreated || terminalProcessSupported"
      },
      {
        "key": "shift+cmd+[",
        "command": "-workbench.action.terminal.focusPrevious",
        "when": "terminalFocus && terminalHasBeenCreated && !terminalEditorFocus || terminalFocus && terminalProcessSupported && !terminalEditorFocus"
      },
      {
        "key": "shift+cmd+]",
        "command": "-workbench.action.debug.nextConsole",
        "when": "inDebugRepl"
      },
      {
        "key": "shift+cmd+]",
        "command": "-workbench.action.terminal.focusNext",
        "when": "terminalFocus && terminalHasBeenCreated && !terminalEditorFocus || terminalFocus && terminalProcessSupported && !terminalEditorFocus"
      },
      {
        "key": "shift+cmd+]",
        "command": "-workbench.action.terminal.focusNext",
        "when": "terminalHasBeenCreated || terminalProcessSupported"
      },
      {
        "key": "cmd+n",
        "command": "-workbench.action.files.newUntitledFile"
      },
      {
        "key": "cmd+n",
        "command": "-editor.action.inlineDiffs.rejectPartialEdit",
        "when": "editorTextFocus && inlineDiffs.activeEditorWithDiffs"
      },
      {
        "key": "cmd+n",
        "command": "-editor.action.sourceAction",
        "when": "editorHasCodeActionsProvider && editorTextFocus && !editorReadonly"
      },
      {
        "key": "escape",
        "command": "workbench.action.focusActiveEditorGroup",
        "when": "sideBarFocus"
      },
      {
        "key": "cmd+u",
        "command": "rust-analyzer.parentModule",
        "when": "editorTextFocus && editorLangId == 'rust'"
      },
      {
        "key": "cmd+i",
        "command": "composerMode.agent"
      },
      {
        "key": "alt+b",
        "command": "gitlens.toggleFileBlame",
        "when": "editorTextFocus"
      }
      // {
      //   "key": "ctrl+j",
      //   "command": "editor.action.triggerSuggest",
      //   "when": "editorHasCompletionItemProvider && textInputFocus && !editorReadonly && !suggestWidgetVisible"
      // },
      // {
      //   "key": "ctrl+j",
      //   "command": "toggleSuggestionDetails",
      //   "when": "suggestWidgetHasFocusedSuggestion && suggestWidgetVisible && textInputFocus"
      // }
      // {
      //   "key": "ctrl+shift+cmd+i",
      //   "command": "editor.action.triggerSuggest",
      //   "when": "editorHasCompletionItemProvider && textInputFocus && !editorReadonly && !suggestWidgetVisible"
      // },
      // {
      //   "key": "ctrl+space",
      //   "command": "-workbench.action.terminal.requestCompletions",
      //   "when": "config.terminal.integrated.suggest.enabled && terminalFocus && terminalProcessSupported && !terminalSuggestWidgetVisible"
      // },
      // {
      //   "key": "ctrl+space",
      //   "command": "-extension.vim_ctrl+space",
      //   "when": "editorTextFocus && vim.active && vim.use<C-space> && !inDebugRepl && vim.mode != 'Insert'"
      // },
      // {
      //   "key": "ctrl+.",
      //   "command": "editor.action.triggerSuggest",
      //   "when": "editorHasCompletionItemProvider && textInputFocus && !editorReadonly && !suggestWidgetVisible"
      // },
      // {
      //   "key": "ctrl+space",
      //   "command": "-workbench.action.terminal.sendSequence",
      //   "when": "terminalFocus && terminalShellIntegrationEnabled && !accessibilityModeEnabled && terminalShellType == 'pwsh'"
      // },
      // {
      //   "key": "ctrl+space",
      //   "command": "-workbench.action.terminal.sendSequence",
      //   "when": "config.terminal.integrated.suggest.enabled && terminalFocus && terminalShellIntegrationEnabled && !accessibilityModeEnabled && terminalShellType == 'pwsh'"
      // },
      // {
      //   "key": "ctrl+space",
      //   "command": "-workbench.action.terminal.suggestToggleDetails",
      //   "when": "simpleSuggestWidgetHasFocusedSuggestion && terminalFocus && terminalHasBeenCreated && terminalIsOpen && terminalSuggestWidgetVisible || simpleSuggestWidgetHasFocusedSuggestion && terminalFocus && terminalIsOpen && terminalProcessSupported && terminalSuggestWidgetVisible"
      // },
      // {
      //   "key": "ctrl+.",
      //   "command": "toggleSuggestionDetails",
      //   "when": "suggestWidgetHasFocusedSuggestion && suggestWidgetVisible && textInputFocus"
      // },
      // {
      //   "key": "ctrl+.",
      //   "command": "quickInput.toggleHover",
      //   "when": "inQuickInput && quickInputType == 'quickPick'"
      // }
    ]  '';
  "zed-settings-json" = ''
    // Zed settings
    //
    // For information on how to configure Zed, see the Zed
    // documentation: https://zed.dev/docs/configuring-zed
    //
    // To see all of Zed's default settings without changing your
    // custom settings, run `zed: open default settings` from the
    // command palette (cmd-shift-p / ctrl-shift-p)
    {
      "code_lens": "off",
      "excerpt_context_lines": 2,
      "expand_excerpt_lines": 5,
      "agent_servers": {
        "claude-acp": {
          "default_config_options": {
            "mode": "bypassPermissions"
          },
          "type": "registry"
        },
        "cursor": {
          "default_config_options": {
            "model": "claude-fable-5"
          },
          "type": "registry"
        },
        "codex-acp": {
          "default_config_options": {
            "fast-mode": true,
            "model": "gpt-5.6-sol",
            "mode": "agent-full-access"
          },
          "type": "registry"
        }
      },
      "indent_guides": {
        "enabled": true
      },
      "inline_code_actions": true,
      "diff_view_style": "split",
      "ui_font_family": "Inter",
      "cli_default_open_behavior": "existing_window",
      "collaboration_panel": {
        "dock": "left",
      },
      "git_panel": {
        "dock": "left",
      },
      "buffer_font_weight": 200.0,
      "terminal": {
        "shell": {
          "program": "nu",
        },
      },
      "agent": {
        "dock": "right",
        "default_model": {
          "effort": "medium",
          "provider": "openai",
          "model": "gpt-5.5",
          "enable_thinking": true,
        },
        "favorite_models": [],
        "model_parameters": [],
      },
      "ssh_connections": [
        {
          "host": "main-zed",
          "args": [],
          "projects": [],
        },
        {
          "host": "main-zed",
          "args": [],
          "projects": [],
        },
      ],
      "telemetry": {
        "diagnostics": true,
        "metrics": false,
      },
      "session": {
        "trust_all_worktrees": true,
      },
      "title_bar": {
        "show_user_menu": false,
        "show_sign_in": false,
        "show_user_picture": false,
        "show_branch_name": true,
        "show_menus": false,
        "show_onboarding_banner": false,
        "show_project_items": true,
      },
      "show_call_status_icon": true,
      "status_bar": {
        "cursor_position_button": true,
        "active_language_button": true,
      },
      "scrollbar": {
        "show": "never",
      },
      "completions": {
        "lsp": true,
        "words": "enabled",
        "lsp_insert_mode": "replace_suffix",
      },
      "file_types": {
        "Nu": ["nu"],
        "Scheme": ["astlog"],
      },
      "lsp": {
        "rust-analyzer": {
          "initialization_options": {
            "check": {
              "command": "clippy",
            },
          },
        },
        // Use Zed's registered nixd adapter, but launch typenix underneath.
        // Wrapper binary `typenix-lsp` runs `typenix --lsp --stdio`.
        "nixd": {
          "binary": {
            "path": "typenix-lsp",
            "arguments": [],
          },
        },
        "basedpyright": {
          "settings": {
            "basedpyright.analysis": {
              "typeCheckingMode": "standard",
            },
          },
        },
      },
      "languages": {
        "Nix": {
          "language_servers": ["nixd", "!nil"],
          "formatter": {
            "external": { "command": "nixfmt", "arguments": [] },
          },
          "format_on_save": "on",
        },
        "Rust": {
          "formatter": "language_server",
          "format_on_save": "on",
        },
        "Python": {
          "language_servers": ["basedpyright", "ruff", "..."],
          "code_actions_on_format": {
            "source.organizeImports.ruff": true,
          },
          "formatter": {
            "language_server": {
              "name": "ruff",
            },
          },
          "format_on_save": "on",
        },
        "TOML": {
          "language_servers": ["crates-lsp", "tombi", "..."],
        },
        "Nu": {
          "language_servers": ["nu", "..."],
        },
      },
      "cursor_blink": false,
      "theme_overrides": {
        "Islands Dark": {
          "players": [
            {
              "cursor": "#808080ff",
              "background": "#808080ff",
              "selection": "#8080803d",
            },
          ],
        },
      },
      "icon_theme": {
        "mode": "dark",
        "light": "JetBrains New UI Icons (Light)",
        "dark": "Catppuccin Frappé",
      },
      "scroll_beyond_last_line": "one_page",
      "vertical_scroll_margin": 20,
      "relative_line_numbers": "enabled",
      "inlay_hints": {
        "show_background": false,
        "enabled": true,
        "show_parameter_hints": false,
        "show_other_hints": true,
      },
      "git": {
        "blame": {
          "show_avatar": true
        },
        "path_style": "file_path_first",
        "inline_blame": {
          "enabled": false,
        },
      },
      "tab_size": 2,
      "auto_update": true,
      "autosave": "on_focus_change",
      "search_wrap": false,
      "ui_font_size": 12.0,
      "use_system_prompts": false,
      "buffer_font_size": 10.0,
      "buffer_line_height": {
        "custom": 1.100000023841858,
      },
      "buffer_font_family": "Berkeley Mono",
      "base_keymap": "JetBrains",
      "gutter": {
        "breakpoints": false,
        "bookmarks": false,
        "folds": false,
        "runnables": false,
        "line_numbers": false,
        "min_line_number_digits": 3,
      },
      "bottom_dock_layout": "contained",
      "active_pane_modifiers": {
        "inactive_opacity": 0.2,
      },
      "auto_install_extensions": {
        "crates-lsp": true,
        "scheme": true,
        "toml": true,
        "tombi": true,
        "gleam": true,
        "nix": true,
        "nu": true,
      },
      "allow_rewrap": "in_comments",
      "centered_layout": {
        "left_padding": 0.0,
        "right_padding": 0.0,
      },
      "toolbar": {
        "breadcrumbs": false,
        "quick_actions": false,
        "code_actions": false,
      },
      "sticky_scroll": {
        "enabled": true,
      },
      "outline_panel": {
        "file_icons": true,
        "auto_fold_dirs": true,
        "auto_reveal_entries": true,
        "button": false,
        "default_width": 240,
        "dock": "left",
        "expand_outlines_with_depth": 100,
        "folder_icons": true,
        "git_status": true,
        "indent_guides": {
          "show": "always",
        },
        "indent_size": 10,
        "scrollbar": {
          "show": "never",
        },
      },
      "close_on_file_delete": true,
      "confirm_quit": false,
      "project_panel": {
        "git_status_indicator": true,
        "diagnostic_badges": true,
        "auto_fold_dirs": true,
        "auto_reveal_entries": true,
        "button": false,
        "default_width": 200,
        "dock": "left",
        "entry_spacing": "standard",
        "file_icons": true,
        "folder_icons": true,
        "git_status": true,
        "hide_gitignore": true,
        "indent_size": 16.0,
        "indent_guides": {
          "show": "always",
        },
        "drag_and_drop": true,
        "show_diagnostics": "all",
        "starts_open": true,
        "scrollbar": {
          "show": "never",
        },
        "hide_root": true,
      },
      "diagnostics": {
        "button": false,
        "include_warnings": true,
        "lsp_pull_diagnostics": {
          "enabled": true,
          "debounce_ms": 50,
        },
        "inline": {
          "enabled": true,
          "update_debounce_ms": 150,
          "padding": 4,
          "min_column": 0,
          "max_severity": null,
        },
      },
      "tabs": {
        "activate_on_close": "neighbour",
        "file_icons": true,
        "close_position": "right",
        "show_close_button": "hidden",
        "git_status": true,
        "show_diagnostics": "all",
      },
      "preview_tabs": {
        "enable_keep_preview_on_code_navigation": true,
        "enable_preview_from_file_finder": true,
        "enabled": true,
      },
      "tab_bar": {
        "show": true,
        "show_tab_bar_buttons": false,
        "show_nav_history_buttons": false,
      },
      "unstable.ui_density": "compact",
      "vim": {
        "toggle_relative_line_numbers": false,
        "highlight_on_yank_duration": 0,
        "use_system_clipboard": "always",
      },
      "vim_mode": true,
      "theme": {
        "mode": "dark",
        "light": "Islands Light",
        "dark": "Islands Dark",
      },
    }
  '';
}
