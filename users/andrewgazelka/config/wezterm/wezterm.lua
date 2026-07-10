local wezterm = require("wezterm")
local config = wezterm.config_builder()

-- Font settings
config.font = wezterm.font("Berkeley Mono")
config.font_size = 10

-- Color scheme (dark theme similar to custom-dark)
config.color_scheme = "Catppuccin Mocha"

-- Window settings
config.window_decorations = "NONE"
config.window_padding = {
	left = 5,
	right = 5,
	top = 5,
	bottom = 5,
}
config.window_close_confirmation = "NeverPrompt"
config.adjust_window_size_when_changing_font_size = false

-- Tab bar
config.enable_tab_bar = true
config.hide_tab_bar_if_only_one_tab = true
config.use_fancy_tab_bar = false

-- Cursor
config.default_cursor_style = "SteadyBlock"

-- Scrollback
config.scrollback_lines = 1000000
config.enable_scroll_bar = false

-- macOS specific
config.macos_window_background_blur = 0
config.native_macos_fullscreen_mode = false
config.send_composed_key_when_left_alt_is_pressed = false
config.send_composed_key_when_right_alt_is_pressed = false

-- Clipboard
config.warn_about_missing_glyphs = false

-- Pane divider
config.inactive_pane_hsb = {
	saturation = 1.0,
	brightness = 1.0,
}

-- Keybindings
config.keys = {
	-- Cmd+Backspace = delete to start of line (Ctrl+U)
	{ key = "Backspace", mods = "CMD", action = wezterm.action.SendString("\x15") },

	-- Cmd+1 = Send F13 for nvim file explorer
	{ key = "1", mods = "CMD", action = wezterm.action.SendString("\x1bOP") },

	-- Tab management
	{ key = "n", mods = "CMD", action = wezterm.action.SpawnWindow },
	{ key = "t", mods = "CMD", action = wezterm.action.SpawnTab("CurrentPaneDomain") },
	{ key = "]", mods = "CMD|SHIFT", action = wezterm.action.ActivateTabRelative(1) },
	{ key = "[", mods = "CMD|SHIFT", action = wezterm.action.ActivateTabRelative(-1) },
	{ key = "w", mods = "CMD", action = wezterm.action.CloseCurrentPane({ confirm = false }) },

	-- Font size
	{ key = "0", mods = "CMD", action = wezterm.action.ResetFontSize },
	{ key = "+", mods = "CMD", action = wezterm.action.IncreaseFontSize },
	{ key = "-", mods = "CMD", action = wezterm.action.DecreaseFontSize },

	-- Reload config
	{ key = "r", mods = "CMD|SHIFT", action = wezterm.action.ReloadConfiguration },

	-- Pane navigation - Sequential
	{ key = "]", mods = "CMD", action = wezterm.action.ActivatePaneDirection("Next") },
	{ key = "[", mods = "CMD", action = wezterm.action.ActivatePaneDirection("Prev") },
	{ key = "Tab", mods = "ALT", action = wezterm.action.ActivatePaneDirection("Next") },
	{ key = "Tab", mods = "ALT|SHIFT", action = wezterm.action.ActivatePaneDirection("Prev") },

	-- Pane navigation - Directional
	{ key = "LeftArrow", mods = "CMD|SHIFT", action = wezterm.action.ActivatePaneDirection("Left") },
	{ key = "RightArrow", mods = "CMD|SHIFT", action = wezterm.action.ActivatePaneDirection("Right") },
	{ key = "UpArrow", mods = "CMD|SHIFT", action = wezterm.action.ActivatePaneDirection("Up") },
	{ key = "DownArrow", mods = "CMD|SHIFT", action = wezterm.action.ActivatePaneDirection("Down") },

	-- Scrollback navigation
	{ key = "UpArrow", mods = "CMD", action = wezterm.action.ScrollToTop },
	{ key = "DownArrow", mods = "CMD", action = wezterm.action.ScrollToBottom },

	-- Shift+Enter
	{ key = "Enter", mods = "SHIFT", action = wezterm.action.SendString("\x1b\r") },

	-- Unbind Cmd+A (like Ghostty)
	{ key = "a", mods = "CMD", action = wezterm.action.DisableDefaultAssignment },
}

return config
