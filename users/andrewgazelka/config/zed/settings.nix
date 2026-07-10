{
  active_pane_modifiers = {
    inactive_opacity = 0.2;
  };
  agent = {
    default_model = {
      effort = "medium";
      enable_thinking = true;
      model = "gpt-5.5";
      provider = "openai";
    };
    dock = "right";
    favorite_models = [];
    model_parameters = [];
  };
  agent_servers = {
    claude-acp = {
      default_config_options = {
        mode = "bypassPermissions";
      };
      type = "registry";
    };
    codex-acp = {
      default_config_options = {
        fast-mode = true;
        mode = "agent-full-access";
        model = "gpt-5.6-sol";
      };
      type = "registry";
    };
    cursor = {
      default_config_options = {
        model = "claude-fable-5";
      };
      type = "registry";
    };
  };
  allow_rewrap = "in_comments";
  auto_install_extensions = {
    crates-lsp = true;
    gleam = true;
    nix = true;
    nu = true;
    scheme = true;
    tombi = true;
    toml = true;
  };
  auto_update = true;
  autosave = "on_focus_change";
  base_keymap = "JetBrains";
  bottom_dock_layout = "contained";
  buffer_font_family = "Berkeley Mono";
  buffer_font_size = 10;
  buffer_font_weight = 200;
  buffer_line_height = {
    custom = 1.100000023841858;
  };
  centered_layout = {
    left_padding = 0;
    right_padding = 0;
  };
  cli_default_open_behavior = "existing_window";
  close_on_file_delete = true;
  code_lens = "off";
  collaboration_panel = {
    dock = "left";
  };
  completions = {
    lsp = true;
    lsp_insert_mode = "replace_suffix";
    words = "enabled";
  };
  confirm_quit = false;
  cursor_blink = false;
  diagnostics = {
    button = false;
    include_warnings = true;
    inline = {
      enabled = true;
      max_severity = null;
      min_column = 0;
      padding = 4;
      update_debounce_ms = 150;
    };
    lsp_pull_diagnostics = {
      debounce_ms = 50;
      enabled = true;
    };
  };
  diff_view_style = "split";
  excerpt_context_lines = 2;
  expand_excerpt_lines = 5;
  file_types = {
    Nu = ["nu"];
    Scheme = ["astlog"];
  };
  git = {
    blame = {
      show_avatar = true;
    };
    inline_blame = {
      enabled = false;
    };
    path_style = "file_path_first";
  };
  git_panel = {
    dock = "left";
  };
  gutter = {
    bookmarks = false;
    breakpoints = false;
    folds = false;
    line_numbers = false;
    min_line_number_digits = 3;
    runnables = false;
  };
  icon_theme = {
    dark = "Catppuccin Frappé";
    light = "JetBrains New UI Icons (Light)";
    mode = "dark";
  };
  indent_guides = {
    enabled = true;
  };
  inlay_hints = {
    enabled = true;
    show_background = false;
    show_other_hints = true;
    show_parameter_hints = false;
  };
  inline_code_actions = true;
  languages = {
    Nix = {
      format_on_save = "on";
      formatter = {
        external = {
          arguments = [];
          command = "nixfmt";
        };
      };
      language_servers = [
        "nixd"
        "!nil"
      ];
    };
    Nu = {
      language_servers = [
        "nu"
        "..."
      ];
    };
    Python = {
      code_actions_on_format = {
        "source.organizeImports.ruff" = true;
      };
      format_on_save = "on";
      formatter = {
        language_server = {
          name = "ruff";
        };
      };
      language_servers = [
        "basedpyright"
        "ruff"
        "..."
      ];
    };
    Rust = {
      format_on_save = "on";
      formatter = "language_server";
    };
    TOML = {
      language_servers = [
        "crates-lsp"
        "tombi"
        "..."
      ];
    };
  };
  lsp = {
    basedpyright = {
      settings = {
        "basedpyright.analysis" = {
          typeCheckingMode = "standard";
        };
      };
    };
    lua-language-server = {
      settings = {
        Lua = {
          diagnostics = {
            globals = ["vim"];
          };
        };
      };
    };
    nixd = {
      binary = {
        arguments = [];
        path = "typenix-lsp";
      };
    };
    rust-analyzer = {
      initialization_options = {
        check = {
          command = "clippy";
        };
      };
    };
  };
  outline_panel = {
    auto_fold_dirs = true;
    auto_reveal_entries = true;
    button = false;
    default_width = 240;
    dock = "left";
    expand_outlines_with_depth = 100;
    file_icons = true;
    folder_icons = true;
    git_status = true;
    indent_guides = {
      show = "always";
    };
    indent_size = 10;
    scrollbar = {
      show = "never";
    };
  };
  preview_tabs = {
    enable_keep_preview_on_code_navigation = true;
    enable_preview_from_file_finder = true;
    enabled = true;
  };
  project_panel = {
    auto_fold_dirs = true;
    auto_reveal_entries = true;
    button = false;
    default_width = 200;
    diagnostic_badges = true;
    dock = "left";
    drag_and_drop = true;
    entry_spacing = "standard";
    file_icons = true;
    folder_icons = true;
    git_status = true;
    git_status_indicator = true;
    hide_gitignore = true;
    hide_root = true;
    indent_guides = {
      show = "always";
    };
    indent_size = 16;
    scrollbar = {
      show = "never";
    };
    show_diagnostics = "all";
    starts_open = true;
  };
  relative_line_numbers = "enabled";
  scroll_beyond_last_line = "one_page";
  scrollbar = {
    show = "never";
  };
  search_wrap = false;
  session = {
    trust_all_worktrees = true;
  };
  show_call_status_icon = true;
  ssh_connections = [
    {
      args = [];
      host = "main-zed";
      projects = [];
    }
    {
      args = [];
      host = "main-zed";
      projects = [];
    }
  ];
  status_bar = {
    active_language_button = true;
    cursor_position_button = true;
  };
  sticky_scroll = {
    enabled = true;
  };
  tab_bar = {
    show = true;
    show_nav_history_buttons = false;
    show_tab_bar_buttons = false;
  };
  tab_size = 2;
  tabs = {
    activate_on_close = "neighbour";
    close_position = "right";
    file_icons = true;
    git_status = true;
    show_close_button = "hidden";
    show_diagnostics = "all";
  };
  telemetry = {
    diagnostics = true;
    metrics = false;
  };
  terminal = {
    shell = {
      program = "nu";
    };
  };
  theme = {
    dark = "Islands Dark";
    light = "Islands Light";
    mode = "dark";
  };
  theme_overrides = {
    "Islands Dark" = {
      players = [
        {
          background = "#808080ff";
          cursor = "#808080ff";
          selection = "#8080803d";
        }
      ];
    };
  };
  title_bar = {
    show_branch_name = true;
    show_menus = false;
    show_onboarding_banner = false;
    show_project_items = true;
    show_sign_in = false;
    show_user_menu = false;
    show_user_picture = false;
  };
  toolbar = {
    breadcrumbs = false;
    code_actions = false;
    quick_actions = false;
  };
  ui_font_family = "Inter";
  ui_font_size = 12;
  "unstable.ui_density" = "compact";
  use_system_prompts = false;
  vertical_scroll_margin = 20;
  vim = {
    highlight_on_yank_duration = 0;
    toggle_relative_line_numbers = false;
    use_system_clipboard = "always";
  };
  vim_mode = true;
}
