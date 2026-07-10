local M = {}

local xdg_state_home = vim.env.XDG_STATE_HOME or (vim.env.HOME .. "/.local/state")
local theme_state_file = xdg_state_home .. "/theme/current"
local active_schemes = {
  cozybox = true,
  ["cozybox-light"] = true,
}

local terminal_palette = {
  dark = {
    "#1d2021",
    "#ea6962",
    "#8ec97c",
    "#d79921",
    "#5b84de",
    "#b16286",
    "#689d6a",
    "#a89984",
    "#928374",
    "#ea6962",
    "#8ec97c",
    "#fabd2f",
    "#5b84de",
    "#d3869b",
    "#8ec07c",
    "#ebdbb2",
  },
  light = {
    "#f9f5d7",
    "#c5524a",
    "#427b58",
    "#d79921",
    "#4261a5",
    "#b16286",
    "#689d6a",
    "#7c6f64",
    "#928374",
    "#c5524a",
    "#427b58",
    "#fabd2f",
    "#4261a5",
    "#d3869b",
    "#8ec07c",
    "#3c3836",
  },
}

local function apply_terminal_palette(mode)
  vim.o.termguicolors = true

  for index, color in ipairs(terminal_palette[mode] or terminal_palette.dark) do
    vim.g["terminal_color_" .. (index - 1)] = color
  end
end

local function ensure_server_socket()
  local socket_path = ("/tmp/nvim-%d.sock"):format(vim.fn.getpid())
  local active_servers = vim.fn.serverlist()

  for _, server in ipairs(active_servers) do
    if server == socket_path then return end
  end

  local stat = vim.uv.fs_stat(socket_path)
  if stat and stat.type == "socket" then vim.fn.delete(socket_path) end

  vim.fn.serverstart(socket_path)
end

local function apply_cozybox_overrides()
  local normal = vim.api.nvim_get_hl(0, { name = "Normal", link = false })
  local cursor_line = vim.api.nvim_get_hl(0, { name = "CursorLine", link = false })
  local visual = vim.api.nvim_get_hl(0, { name = "Visual", link = false })
  local comment = vim.api.nvim_get_hl(0, { name = "Comment", link = false })
  local identifier = vim.api.nvim_get_hl(0, { name = "Identifier", link = false })
  local statement = vim.api.nvim_get_hl(0, { name = "Statement", link = false })
  local menu = { fg = normal.fg, bg = normal.bg }
  local selected = { fg = normal.fg, bg = cursor_line.bg or visual.bg or normal.bg, bold = true }

  vim.api.nvim_set_hl(0, "Pmenu", menu)
  vim.api.nvim_set_hl(0, "PmenuBorder", { fg = comment.fg or normal.fg, bg = normal.bg })
  vim.api.nvim_set_hl(0, "PmenuKind", { fg = identifier.fg or normal.fg, bg = normal.bg })
  vim.api.nvim_set_hl(0, "PmenuKindSel", { fg = identifier.fg or normal.fg, bg = selected.bg, bold = true })
  vim.api.nvim_set_hl(0, "PmenuExtra", { fg = comment.fg or normal.fg, bg = normal.bg })
  vim.api.nvim_set_hl(0, "PmenuExtraSel", { fg = comment.fg or normal.fg, bg = selected.bg, bold = true })
  vim.api.nvim_set_hl(0, "PmenuMatch", { fg = statement.fg or normal.fg, bg = normal.bg, bold = true })
  vim.api.nvim_set_hl(0, "PmenuMatchSel", { fg = statement.fg or normal.fg, bg = selected.bg, bold = true })
  vim.api.nvim_set_hl(0, "PmenuSbar", { bg = selected.bg })
  vim.api.nvim_set_hl(0, "PmenuThumb", { bg = visual.bg or selected.bg })
  vim.api.nvim_set_hl(0, "PmenuSel", selected)

  local links = {
    { "DiffsAdd", "DiffAdd" },
    { "DiffsDelete", "DiffDelete" },
    { "DiffsChange", "DiffChange" },
    { "DiffsText", "DiffText" },
    { "DiffsClear", "Normal" },
  }

  for _, pair in ipairs(links) do
    vim.api.nvim_set_hl(0, pair[1], { link = pair[2], default = false })
  end
end

local function read_mode()
  local ok, lines = pcall(vim.fn.readfile, theme_state_file)
  if not ok or not lines or not lines[1] then return "dark" end

  local mode = vim.trim(lines[1])
  if mode == "light" then return "light" end

  return "dark"
end

local function colorscheme_for_mode(mode)
  if mode == "light" then return "cozybox-light" end

  return "cozybox"
end

function M.apply(mode)
  local next_mode = mode or read_mode()
  local next_scheme = colorscheme_for_mode(next_mode)

  if vim.o.background ~= next_mode then vim.o.background = next_mode end

  if vim.g.cozybox_theme_mode ~= next_mode or vim.g.colors_name ~= next_scheme then vim.cmd.colorscheme(next_scheme) end

  vim.g.cozybox_theme_mode = next_mode
  apply_terminal_palette(next_mode)
  apply_cozybox_overrides()
  local ok_reload, fzf_reload = pcall(require, "config.fzf_reload")
  if ok_reload then pcall(fzf_reload.reload) end
  vim.schedule(function() pcall(vim.cmd, "redrawstatus!") end)
end

function M.setup()
  local group = vim.api.nvim_create_augroup("cozybox_theme_sync", { clear = true })

  ensure_server_socket()

  vim.api.nvim_create_autocmd("ColorScheme", {
    group = group,
    callback = function()
      if active_schemes[vim.g.colors_name] then
        apply_terminal_palette(vim.o.background == "light" and "light" or "dark")
        apply_cozybox_overrides()
      end
    end,
  })

  vim.api.nvim_create_autocmd({ "VimEnter", "FocusGained" }, {
    group = group,
    callback = function() M.apply() end,
  })

  vim.api.nvim_create_user_command("ThemeSync", function(opts)
    local mode = opts.args ~= "" and opts.args or nil
    M.apply(mode)
  end, {
    nargs = "?",
    complete = function() return { "dark", "light" } end,
  })

  M.apply()
end

return M
