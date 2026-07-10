# Zellij configuration and plugin wiring. Home Manager renders the structured
# settings and layouts to KDL with lib.hm.generators.toKDL.
{
  configRoot,
  lib,
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

  command = name: args: {${name} = args;};
  bind = keys: actions: {
    bind = {
      _args = keys;
      _children = actions;
    };
  };
  mode = bindings: {_children = bindings;};
  palette = base: background: emphasis: {
    inherit base background;
    emphasis_0 = emphasis;
    emphasis_1 = emphasis;
    emphasis_2 = emphasis;
    emphasis_3 = emphasis;
  };
  normal = command "SwitchToMode" ["Normal"];
in {
  settings = {
    themes.minimal = {
      ribbon_selected = palette [30 30 30] [160 160 160] [160 160 160];
      ribbon_unselected = palette [120 120 120] [45 45 45] [45 45 45];
      text_unselected = palette [160 160 160] [26 26 26] [120 120 120];
      text_selected = palette [200 200 200] [40 40 40] [140 140 140];
      frame_selected = palette [100 100 100] [0] [80 80 80];
      frame_highlight = palette [140 140 140] [0] [100 100 100];
      exit_code_success = palette [120 160 120] [0] [100 100 100];
      exit_code_error = palette [180 100 100] [0] [100 100 100];
      table_title = palette [140 140 140] [0] [100 100 100];
      table_cell_selected = palette [200 200 200] [50 50 50] [140 140 140];
      table_cell_unselected = palette [160 160 160] [30 30 30] [120 120 120];
      list_selected = palette [200 200 200] [50 50 50] [140 140 140];
      list_unselected = palette [160 160 160] [30 30 30] [120 120 120];
      multiplayer_user_colors = lib.genAttrs (map (n: "player_${toString n}") (lib.range 1 10)) (
        name: let
          number = lib.toInt (lib.removePrefix "player_" name);
          shade =
            if number == 1
            then 150
            else if number == 2
            then 130
            else 140 - (number * 10);
        in [shade shade shade]
      );
    };
    theme = "minimal";

    ui.pane_frames = {
      rounded_corners = true;
      hide_session_name = true;
    };
    simplified_ui = true;
    pane_frames = false;
    default_shell = "nu";
    copy_on_select = true;
    copy_command = "pbcopy";
    mouse_mode = true;
    env = {
      TERM = "xterm-256color";
      COLORTERM = "truecolor";
    };
    session_serialization = true;
    serialize_pane_viewport = true;
    scrollback_editor = "nvim";
    scroll_buffer_size = 50000;
    show_startup_tips = false;
    default_layout = "compact";

    keybinds = {
      _props.clear-defaults = true;
      locked = mode [
        (bind ["Ctrl g"] [(command "SwitchToMode" ["Normal"])])
      ];
      normal = mode [
        (bind ["Ctrl g"] [(command "SwitchToMode" ["Locked"])])
        (bind ["Ctrl h"] [(command "MoveFocus" ["Left"])])
        (bind ["Ctrl j"] [(command "MoveFocus" ["Down"])])
        (bind ["Ctrl k"] [(command "MoveFocus" ["Up"])])
        (bind ["Ctrl l"] [(command "MoveFocus" ["Right"])])
        (bind ["Ctrl n"] [(command "GoToNextTab" [])])
        (bind ["Ctrl p"] [(command "GoToPreviousTab" [])])
        (bind ["Ctrl \\"] [(command "NewPane" ["Right"])])
        (bind ["Ctrl -"] [(command "NewPane" ["Down"])])
        (bind ["Ctrl x"] [(command "CloseFocus" [])])
        (bind ["Ctrl z"] [(command "ToggleFocusFullscreen" [])])
        (bind ["Ctrl f"] [(command "ToggleFloatingPanes" [])])
        (bind ["Ctrl Space"] [(command "SwitchToMode" ["Pane"])])
        (bind ["Ctrl t"] [(command "SwitchToMode" ["Tab"])])
        (bind ["Ctrl s"] [(command "SwitchToMode" ["Scroll"])])
        (bind ["Ctrl r"] [(command "SwitchToMode" ["Resize"])])
        (bind ["Ctrl o"] [(command "SwitchToMode" ["Session"])])
      ];
      pane = mode [
        (bind ["Esc" "Ctrl c"] [normal])
        (bind ["h" "Left"] [(command "MoveFocus" ["Left"]) normal])
        (bind ["j" "Down"] [(command "MoveFocus" ["Down"]) normal])
        (bind ["k" "Up"] [(command "MoveFocus" ["Up"]) normal])
        (bind ["l" "Right"] [(command "MoveFocus" ["Right"]) normal])
        (bind ["n"] [(command "NewPane" []) normal])
        (bind ["\\" "|"] [(command "NewPane" ["Right"]) normal])
        (bind ["-" "_"] [(command "NewPane" ["Down"]) normal])
        (bind ["x"] [(command "CloseFocus" []) normal])
        (bind ["z" "f"] [(command "ToggleFocusFullscreen" []) normal])
        (bind ["w"] [(command "ToggleFloatingPanes" []) normal])
        (bind ["e"] [(command "TogglePaneEmbedOrFloating" []) normal])
        (bind ["r"] [(command "SwitchToMode" ["RenamePane"]) (command "PaneNameInput" [0])])
      ];
      tab = mode (
        [
          (bind ["Esc" "Ctrl c"] [normal])
          (bind ["h" "Left"] [(command "GoToPreviousTab" [])])
          (bind ["l" "Right"] [(command "GoToNextTab" [])])
          (bind ["j" "Down"] [(command "GoToNextTab" [])])
          (bind ["k" "Up"] [(command "GoToPreviousTab" [])])
        ]
        ++ map (number: bind [(toString number)] [(command "GoToTab" [number]) normal]) (lib.range 1 9)
        ++ [
          (bind ["n"] [(command "NewTab" []) normal])
          (bind ["x"] [(command "CloseTab" []) normal])
          (bind ["r"] [(command "SwitchToMode" ["RenameTab"]) (command "TabNameInput" [0])])
          (bind ["s"] [(command "ToggleActiveSyncTab" []) normal])
          (bind ["b"] [(command "BreakPane" []) normal])
        ]
      );
      resize = mode [
        (bind ["Esc" "Ctrl c"] [normal])
        (bind ["h" "Left"] [(command "Resize" ["Increase Left"])])
        (bind ["j" "Down"] [(command "Resize" ["Increase Down"])])
        (bind ["k" "Up"] [(command "Resize" ["Increase Up"])])
        (bind ["l" "Right"] [(command "Resize" ["Increase Right"])])
        (bind ["H"] [(command "Resize" ["Decrease Left"])])
        (bind ["J"] [(command "Resize" ["Decrease Down"])])
        (bind ["K"] [(command "Resize" ["Decrease Up"])])
        (bind ["L"] [(command "Resize" ["Decrease Right"])])
        (bind ["=" "+"] [(command "Resize" ["Increase"])])
        (bind ["-"] [(command "Resize" ["Decrease"])])
      ];
      scroll = mode [
        (bind ["Esc" "Ctrl c"] [normal])
        (bind ["j" "Down"] [(command "ScrollDown" [])])
        (bind ["k" "Up"] [(command "ScrollUp" [])])
        (bind ["d" "Ctrl d"] [(command "HalfPageScrollDown" [])])
        (bind ["u" "Ctrl u"] [(command "HalfPageScrollUp" [])])
        (bind ["f" "Ctrl f"] [(command "PageScrollDown" [])])
        (bind ["b" "Ctrl b"] [(command "PageScrollUp" [])])
        (bind ["g"] [(command "ScrollToTop" [])])
        (bind ["G"] [(command "ScrollToBottom" [])])
        (bind ["/"] [(command "SwitchToMode" ["EnterSearch"]) (command "SearchInput" [0])])
        (bind ["n"] [(command "Search" ["down"])])
        (bind ["N"] [(command "Search" ["up"])])
        (bind ["e"] [(command "EditScrollback" []) normal])
      ];
      search = mode [
        (bind ["Esc" "Ctrl c"] [normal])
        (bind ["j" "Down"] [(command "ScrollDown" [])])
        (bind ["k" "Up"] [(command "ScrollUp" [])])
        (bind ["d" "Ctrl d"] [(command "HalfPageScrollDown" [])])
        (bind ["u" "Ctrl u"] [(command "HalfPageScrollUp" [])])
        (bind ["n"] [(command "Search" ["down"])])
        (bind ["N"] [(command "Search" ["up"])])
        (bind ["c"] [(command "SearchToggleOption" ["CaseSensitivity"])])
        (bind ["w"] [(command "SearchToggleOption" ["Wrap"])])
        (bind ["o"] [(command "SearchToggleOption" ["WholeWord"])])
      ];
      entersearch = mode [
        (bind ["Esc" "Ctrl c"] [(command "SwitchToMode" ["Scroll"])])
        (bind ["Enter"] [(command "SwitchToMode" ["Search"])])
      ];
      session = mode [
        (bind ["Esc" "Ctrl c"] [normal])
        (bind ["d"] [(command "Detach" [])])
        (bind ["w"] [
          {
            LaunchOrFocusPlugin = {
              _args = ["session-manager"];
              floating = true;
              move_to_focused_tab = true;
            };
          }
          normal
        ])
      ];
      renametab = mode [
        (bind ["Esc" "Ctrl c"] [(command "UndoRenameTab" []) (command "SwitchToMode" ["Tab"])])
        (bind ["Enter"] [normal])
      ];
      renamepane = mode [
        (bind ["Esc" "Ctrl c"] [(command "UndoRenamePane" []) (command "SwitchToMode" ["Pane"])])
        (bind ["Enter"] [normal])
      ];
      shared_except = {
        _args = ["locked"];
        _children = [(bind ["Ctrl q"] [(command "Quit" [])])];
      };
    };

    plugins = {
      "tab-bar"._props.location = "zellij:tab-bar";
      "status-bar"._props.location = "zellij:status-bar";
      strider._props.location = "zellij:strider";
      "compact-bar" = {
        _props.location = "zellij:compact-bar";
        tooltip = "F1";
      };
      "session-manager"._props.location = "zellij:session-manager";
      filepicker = {
        _props.location = "zellij:strider";
        cwd = "/";
      };
    };
  };

  layouts.minimal.layout.default_tab_template._children = [
    {
      pane = {
        _props = {
          size = 1;
          borderless = true;
        };
        plugin._props.location = "file:${xdgConfigHome}/zellij/plugins/minimal-tab-bar.wasm";
      };
    }
    {children = {};}
  ];
  plugins = [
    minimalTabBar
    zellijPlugins.zjstatus
  ];
}
