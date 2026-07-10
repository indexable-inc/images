{
  set = {
    default-terminal = "tmux-256color";
    alternate-screen = "on";
    allow-passthrough = "on";
    mouse = "on";
    base-index = "1";
    pane-base-index = "1";
    renumber-windows = "on";
    automatic-rename = "off";
    allow-rename = "on";
    set-titles = "on";
    escape-time = "0";
    default-shell = "/bin/zsh";
    default-command = "/bin/zsh";
    history-limit = "50000";
    display-time = "4000";
    focus-events = "on";
    aggressive-resize = "on";
    visual-activity = "off";
    visual-bell = "off";
    visual-silence = "off";
    activity-action = "other";
    bell-action = "other";
    silence-action = "none";
    set-clipboard = "on";
    status-position = "bottom";
    status-style = "bg=#1c1c1c fg=#d0d0d0";
    status-left-length = "40";
    status-right-length = "60";
    status-justify = "left";
    status-interval = "1";
    status-left = "#{?client_prefix,#[fg=colour234 bg=colour108] PREFIX ,}";
    status-right = "#[fg=#5f5f5f]#{?synchronize-panes,#[fg=#d75f5f,bold] SYNC ,}#[fg=#808080]#{pane_width}x#{pane_height} #[fg=#a8a8a8]#{cursor_x}:#{cursor_y} ";
    window-status-current-format = "#{?window_bell_flag,#[fg=#1c1c1c bg=#d46a6a],#[fg=#1c1c1c bg=#e0e0e0]}#[bold] #I #[bg=default] #{?window_bell_flag,#[fg=#e57373],#[fg=#ffffff]}#{b:pane_current_path} #W#{?window_zoomed_flag,#[fg=#d4a656] 󰊓,}";
    window-status-format = "#{?window_bell_flag,#[fg=#1c1c1c bg=#d46a6a],#[fg=#c0c0c0 bg=#333333]} #I #[bg=default] #{?window_bell_flag,#[fg=#e57373],#[fg=#888888]}#{b:pane_current_path} #W";
    window-status-separator = "  ";
    window-status-bell-style = "fg=colour167,bold";
    pane-border-style = "fg=#3a3a3a";
    pane-active-border-style = "fg=#c0c0c0";
    pane-border-status = "off";
    message-style = "fg=#e4e4e4,bg=#303030,bold";
    message-command-style = "fg=#e4e4e4,bg=#444444";
    mode-style = "fg=#1c1c1c,bg=#87af87,bold";
  };
  server = {
    extended-keys = "on";
    "user-keys[0]" = "\\e[13;9u";
  };
  appendServer.terminal-features = [
    "xterm-ghostty:extkeys"
    ",xterm-256color:RGB:256:clipboard:ccolour:cstyle:extkeys:focus:hyperlinks:margins:overline:rectfill:strikethrough:sync:title:usstyle"
    ",xterm-ghostty:RGB:256:clipboard:ccolour:cstyle:extkeys:focus:hyperlinks:margins:overline:rectfill:sixel:strikethrough:sync:title:usstyle"
    ",xterm-kitty:RGB:256:clipboard:ccolour:cstyle:extkeys:focus:hyperlinks:margins:overline:rectfill:strikethrough:sync:title:usstyle"
  ];
  window = {
    monitor-activity = "on";
    monitor-bell = "on";
    mode-keys = "vi";
  };
  plugins = [
    "tmux-plugins/tpm"
    "jabirali/tmux-tilish"
    "nhdaly/tmux-better-mouse-mode"
    "rickstaa/tmux-notify"
  ];
  pluginSettings = {
    scroll-speed-num-lines-per-scroll = "3";
    tnotify-verbose = "on";
    tnotify-sleep-duration = "5";
    tilish-default = "main-vertical";
    tilish-navigator = "on";
  };
  bindings = [
    {
      key = "c";
      command = "new-window -c '#{pane_current_path}'";
    }
    {
      key = "\"";
      command = "split-window -c '#{pane_current_path}'";
    }
    {
      key = "%";
      command = "split-window -h -c '#{pane_current_path}'";
    }
    {
      key = "r";
      command = "source-file ~/.tmux.conf \\; display-message 'Config reloaded!'";
    }
    {
      key = "e";
      command = "run-shell \"tmux capture-pane -pS - > /tmp/tmux-buffer && nvim '+ normal G' /tmp/tmux-buffer\"";
    }
    {
      key = "M-e";
      command = "run-shell \"tmux capture-pane -pS - > /tmp/tmux-buffer && nvim '+ normal G' /tmp/tmux-buffer\"";
      noPrefix = true;
    }
    {
      key = "M-k";
      command = "clear-history \\; display-message 'History cleared!'";
      noPrefix = true;
    }
    {
      key = "m";
      command = "setw monitor-activity \\; display-message 'Activity monitoring: #{?monitor-activity,ON,OFF}'";
    }
    {
      key = "M";
      command = "setw monitor-silence 5 \\; display-message 'Silence monitoring: 5s'";
    }
    {
      key = "M-M";
      command = "setw monitor-silence 0 \\; display-message 'Silence monitoring: OFF'";
      noPrefix = true;
    }
    {
      key = "v";
      command = "send-keys -X begin-selection";
      table = "copy-mode-vi";
    }
    {
      key = "y";
      command = "send-keys -X copy-pipe-and-cancel 'pbcopy'";
      table = "copy-mode-vi";
    }
    {
      key = "C-v";
      command = "send-keys -X rectangle-toggle";
      table = "copy-mode-vi";
    }
    {
      key = "Escape";
      command = "send-keys -X cancel";
      table = "copy-mode-vi";
    }
    {
      key = "User0";
      command = "split-window -c '#{pane_current_path}'";
      noPrefix = true;
    }
  ];
  unbindNoPrefix = ["M-Enter"];
  run = ["~/.tmux/plugins/tpm/tpm"];
}
