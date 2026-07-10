local core = require "mux.core"
local project = require "mux.project"
local session = require "mux.session"

local M = {}

local canon = core.canon
local find_view = core.find_view
local views = core.views
local VIEW_ORDER = core.VIEW_ORDER
local TABLINE_EXPR = "%!v:lua.require'mux.line'.render()"
local refresh_pending = false

-- cozybox palette (mirrors lib/theme.nix); keyed by vim.o.background so the bar
-- tracks `theme` switches. Purple accent + muted separators, matching the old
-- tmux status bar.
local PALETTE = {
  dark = { purple = "#d3869b", white = "#ebdbb2", muted = "#7c6f64", border = "#3c3836", bg = "#101010" },
  light = { purple = "#d3869b", white = "#3c3836", muted = "#665c54", border = "#000000", bg = "#e7e7e7" },
}

function M.setup_hl()
  local c = vim.o.background == "light" and PALETTE.light or PALETTE.dark
  local set = vim.api.nvim_set_hl
  set(0, "MuxFill", { fg = c.muted, bg = c.bg })
  set(0, "MuxText", { fg = c.white, bg = c.bg })
  set(0, "MuxTextCur", { fg = c.white, bg = c.bg, bold = true })
  set(0, "MuxAccent", { fg = c.purple, bg = c.bg })
  set(0, "MuxMark", { fg = c.purple, bg = c.bg, bold = true })
  set(0, "MuxMuted", { fg = c.muted, bg = c.bg })
  set(0, "TabLineFill", { link = "MuxFill" })
  set(0, "WinSeparator", { fg = c.border })
end

local function visibility_file() return core.runtime_dir() .. "/bar" end

local function visibility_mode()
  local file = visibility_file()
  if vim.fn.filereadable(file) == 1 then
    local mode = vim.fn.readfile(file)[1]
    if mode == "hide" then return "hide" end
  end
  return "show"
end

local function write_visibility(mode)
  pcall(vim.fn.mkdir, core.runtime_dir(), "p")
  pcall(vim.fn.writefile, { mode }, visibility_file())
end

---@param group string
---@param text string
---@return string
local function hl(group, text)
  if text == "" then return "" end
  return ("%%#%s#%s%%*"):format(group, text)
end

-- window/session mark: `*` current (accent), else nothing.
---@param current boolean
---@return string
local function mark(current)
  if current then return hl("MuxMark", "*") end
  return ""
end

---@param key string
---@param name string
---@param current boolean
---@return string
local function view_segment(key, name, current)
  local body = current and "MuxTextCur" or "MuxText"
  return mark(current) .. hl(body, key) .. hl("MuxAccent", ":") .. hl(body, name)
end

---@param name string
---@param current boolean
---@return string
local function session_segment(name, current)
  local body = current and "MuxTextCur" or "MuxText"
  return mark(current) .. hl(body, name)
end

---@param tp integer
---@return string label like tmux automatic-rename: basename of the terminal cwd,
---falling back to the tab's cwd
local function window_label(tp)
  local win = vim.api.nvim_tabpage_get_win(tp)
  local dir = core.terminal_cwd(vim.api.nvim_win_get_buf(win))
  if not dir or dir == "" then
    local ok, cwd = pcall(vim.fn.getcwd, vim.api.nvim_win_get_number(win), vim.api.nvim_tabpage_get_number(tp))
    dir = (ok and cwd) or ""
  end
  local name = dir ~= "" and vim.fn.fnamemodify(dir, ":t") or ""
  if name == "" then name = dir == "/" and "/" or "term" end
  return name
end

---@return string[]
local function view_segments()
  core.prune()
  local current = vim.api.nvim_get_current_tabpage()
  local parts = {}
  for _, name in ipairs(VIEW_ORDER) do
    local tp = find_view(name)
    if tp then
      local spec = views[name]
      parts[#parts + 1] = view_segment(spec.key, name, tp == current)
    end
  end
  for _, tp in ipairs(vim.api.nvim_list_tabpages()) do
    if not core.tab_view[tp] then
      parts[#parts + 1] = view_segment(tostring(vim.api.nvim_tabpage_get_number(tp)), window_label(tp), tp == current)
    end
  end
  return parts
end

---@return string[]
local function session_segments()
  local entries = project.list_entries()
  local current = session.root()
  local parts = {}
  for _, entry in ipairs(entries) do
    if entry.status == "live" then
      local root = canon(entry.cwd)
      local name = vim.fn.fnamemodify(root, ":t")
      if name ~= "" then parts[#parts + 1] = session_segment(name, root == current) end
    end
  end
  return parts
end

---@param win integer
---@param value string
local function set_winbar(win, value) pcall(vim.api.nvim_set_option_value, "winbar", value, { win = win }) end

local function clear_separator()
  local wins = vim.api.nvim_tabpage_list_wins(0)
  for _, win in ipairs(wins) do
    if vim.api.nvim_win_is_valid(win) and vim.api.nvim_win_get_config(win).relative == "" then set_winbar(win, "") end
  end
end

function M.apply_visibility()
  if vim.env.MUX ~= "1" then return end
  local show = visibility_mode() ~= "hide"
  if vim.o.tabline ~= TABLINE_EXPR then vim.o.tabline = TABLINE_EXPR end
  vim.o.showtabline = show and 2 or 0
  clear_separator()
end

---@return string
function M.render()
  if vim.env.MUX ~= "1" then return "" end
  local divider = hl("MuxMuted", " | ")
  local ok, rendered = pcall(
    function()
      return (" %s%%=%s "):format(table.concat(view_segments(), divider), table.concat(session_segments(), "  "))
    end
  )
  if ok then return rendered end
  return ""
end

function M.refresh()
  if vim.in_fast_event() then
    vim.schedule(M.refresh)
    return
  end
  if vim.env.MUX ~= "1" or refresh_pending then return end
  refresh_pending = true
  vim.schedule(function()
    refresh_pending = false
    M.apply_visibility()
    pcall(vim.cmd.redrawtabline)
    pcall(vim.cmd.redrawstatus)
  end)
end

function M.toggle()
  if vim.env.MUX ~= "1" then return end
  write_visibility(visibility_mode() == "show" and "hide" or "show")
  M.apply_visibility()
  M.refresh()
end

function M.start_watchers()
  if vim.env.MUX ~= "1" or M._timer then return end
  M._timer = vim.uv.new_timer()
  M._timer:start(500, 500, vim.schedule_wrap(function() M.refresh() end))
end

function M.stop_watchers()
  local timer = M._timer
  M._timer = nil
  if timer then
    pcall(timer.stop, timer)
    pcall(timer.close, timer)
  end
end

return M
