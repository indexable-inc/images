local core = require "mux.core"

local views = core.views
local tab_view = core.tab_view
local tag = core.tag
local find_view = core.find_view

local M = {}

---@param cmd string[]
---@param cwd? string
local function start_terminal(cmd, cwd)
  -- jobstart(term=true) converts the current buffer, which a split shares
  -- with the previous window: a modified file is E5108, an existing terminal
  -- can't be converted again, and a clean file buffer would be silently
  -- hijacked. Hand it a fresh scratch buffer instead. The implicit
  -- terminal-mode exit on the buffer swap is marked programmatic so the
  -- pane we split from keeps its insert intent (see plugin/autocmds.lua).
  local prev = vim.api.nvim_get_current_buf()
  if vim.bo[prev].buftype == "terminal" and vim.fn.mode() == "t" then vim.b[prev].term_programmatic = true end
  vim.api.nvim_win_set_buf(0, vim.api.nvim_create_buf(false, true))
  vim.fn.jobstart(cmd, { term = true, cwd = cwd or vim.fn.getcwd() })
  vim.b.term_insert = true
  if #vim.fn.win_findbuf(prev) == 0 and vim.api.nvim_buf_get_name(prev) == "" and not vim.bo[prev].modified then
    pcall(vim.api.nvim_buf_delete, prev, { force = true })
  end
  vim.schedule(core.restore_terminal_focus)
end

local function current_shell_cwd()
  local buf = vim.api.nvim_get_current_buf()
  return core.terminal_cwd(buf) or vim.fn.getcwd()
end

---@param tp integer
---@param restoring? boolean true when pruning a stale tab during session restore
function M.close_view_tab(tp, restoring)
  vim.schedule(function()
    if not vim.api.nvim_tabpage_is_valid(tp) then return end
    if #vim.api.nvim_list_tabpages() <= 1 then
      -- The last tabpage can't be closed. During restore, keep the server
      -- up by resetting this lone tab to a clean edit view and wiping the
      -- dead terminal. Otherwise soft-stop this session and hop to the
      -- latest live project.
      if restoring then
        local dead = {}
        for _, w in ipairs(vim.api.nvim_tabpage_list_wins(tp)) do
          local b = vim.api.nvim_win_get_buf(w)
          if vim.bo[b].buftype == "terminal" then dead[b] = true end
        end
        local keep = vim.api.nvim_tabpage_get_win(tp)
        vim.api.nvim_win_call(keep, function()
          pcall(vim.cmd, "silent! only")
          pcall(vim.cmd.edit, vim.fn.getcwd())
        end)
        for b in pairs(dead) do
          if vim.api.nvim_buf_is_valid(b) then pcall(vim.api.nvim_buf_delete, b, { force = true }) end
        end
        tab_view[tp] = "edit"
        return
      end
      require("mux.project").stop_to_latest()
      return
    end
    local bufs = {}
    for _, w in ipairs(vim.api.nvim_tabpage_list_wins(tp)) do
      bufs[#bufs + 1] = vim.api.nvim_win_get_buf(w)
    end
    local cur = vim.api.nvim_get_current_tabpage()
    local ok = pcall(vim.api.nvim_set_current_tabpage, tp)
    if ok then pcall(vim.cmd, "tabclose") end
    local closed = not vim.api.nvim_tabpage_is_valid(tp)
    if closed then
      tab_view[tp] = nil
      for _, b in ipairs(bufs) do
        if vim.api.nvim_buf_is_valid(b) and vim.bo[b].buftype == "terminal" and #vim.fn.win_findbuf(b) == 0 then
          pcall(vim.api.nvim_buf_delete, b, { force = true })
        end
      end
    end
    if vim.api.nvim_tabpage_is_valid(cur) then pcall(vim.api.nvim_set_current_tabpage, cur) end
    core.restore_terminal_focus()
  end)
end

function M.close_view() M.close_view_tab(vim.api.nvim_get_current_tabpage()) end

-- Close the current window like tmux kill-pane: wipe its terminal buffer,
-- close the tab when it was the last window.
function M.kill_pane()
  local win = vim.api.nvim_get_current_win()
  local buf = vim.api.nvim_win_get_buf(win)
  if core.last_window(win) then
    M.close_view_tab(vim.api.nvim_win_get_tabpage(win))
    return
  end
  pcall(vim.api.nvim_win_close, win, true)
  if vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].buftype == "terminal" and #vim.fn.win_findbuf(buf) == 0 then
    pcall(vim.api.nvim_buf_delete, buf, { force = true })
  end
  core.restore_terminal_focus()
end

---@param win integer window of a dead terminal
---@param buf integer the dead terminal buffer
function M.on_terminal_exit(win, buf)
  if core.last_window(win) then
    M.close_view_tab(vim.api.nvim_win_get_tabpage(win))
    return
  end
  vim.schedule(function()
    if vim.api.nvim_win_is_valid(win) then pcall(vim.api.nvim_win_close, win, true) end
    if vim.api.nvim_buf_is_valid(buf) and #vim.fn.win_findbuf(buf) == 0 then
      pcall(vim.api.nvim_buf_delete, buf, { force = true })
    end
  end)
end

---@param name string
function M.materialize(name)
  local spec = views[name]
  local cwd = vim.fn.getcwd()
  if spec.kind == "editor" then
    vim.cmd.edit(cwd)
  elseif spec.kind == "vcs" then
    pcall(vim.cmd, "Git|only")
  elseif spec.kind == "terminal" then
    start_terminal(spec.cmd, cwd)
  end
end

---@param name string
---@param enter boolean
---@return integer tabpage
local function create_view(name, enter)
  local buf = vim.api.nvim_create_buf(false, true)
  local tp = vim.api.nvim_open_tabpage(buf, enter, {})
  tag(tp, name)
  local win = vim.api.nvim_tabpage_get_win(tp)
  vim.api.nvim_win_call(win, function() M.materialize(name) end)
  return tp
end

-- tmux split-window / new-window: never leave terminal-mode on the way to a
-- new terminal. A stopinsert issued inside a t-mode mapping is deferred past
-- the mapping, cancels every startinsert queued meanwhile (TermOpen,
-- start_terminal), and its TermLeave lands on the new buffer, clearing
-- term_insert. Staying in terminal-mode carries insert straight over.
---@param vertical boolean
function M.split_terminal(vertical)
  local cwd = current_shell_cwd()
  vim.cmd(vertical and "vsplit" or "split")
  start_terminal({ vim.o.shell }, cwd)
end

function M.new_window()
  local cwd = current_shell_cwd()
  vim.cmd "tabnew"
  start_terminal({ vim.o.shell }, cwd)
end

-- tmux-style pane zoom: maximize the current window within its tab and toggle
-- back. Stays inside the tabpage (no new tab), so the mux window model and bar
-- are unaffected.
function M.toggle_zoom()
  if vim.t.mux_zoom then
    local restore = vim.t.mux_zoom
    vim.t.mux_zoom = nil
    pcall(vim.cmd, restore)
  elseif vim.fn.winnr "$" > 1 then
    vim.t.mux_zoom = vim.fn.winrestcmd()
    vim.cmd "wincmd _"
    vim.cmd "wincmd |"
  end
  core.restore_terminal_focus()
end

---@param name string
function M.open_view(name)
  local spec = views[name]
  if not spec then return end
  local terminal = spec.kind == "terminal"
  if not terminal then core.leave_terminal() end
  local existing = find_view(name)
  if existing then
    vim.api.nvim_set_current_tabpage(existing)
    core.restore_terminal_focus()
    return
  end

  create_view(name, true)
end

---@param spec string|{ view?: string, win?: integer, tab?: integer, create?: boolean }
---@return integer? win
---@return string? err
function M.resolve_view(spec)
  if type(spec) == "string" then spec = { view = spec } end
  spec = spec or {}

  if spec.win ~= nil then
    local win = tonumber(spec.win)
    if not (win and vim.api.nvim_win_is_valid(win)) then return nil, "invalid window: " .. tostring(spec.win) end
    return win
  end

  if spec.tab ~= nil then
    local tp = vim.api.nvim_list_tabpages()[tonumber(spec.tab) or -1]
    if not (tp and vim.api.nvim_tabpage_is_valid(tp)) then return nil, "invalid tab: " .. tostring(spec.tab) end
    return vim.api.nvim_tabpage_get_win(tp)
  end

  local name = spec.view
  if not name then return nil, "resolve_view: need view, win, or tab" end
  if not views[name] then return nil, "unknown view: " .. tostring(name) end
  local tp = find_view(name)
  if not tp then
    if spec.create == false then return nil, "view not open: " .. name end
    local saved_alt = M._alt
    tp = create_view(name, false)
    core.restore_terminal_focus()
    M._alt = saved_alt
  end
  return vim.api.nvim_tabpage_get_win(tp)
end

---@param spec string|table
---@param fn fun(): any
---@return any result, string? err
function M.in_view(spec, fn)
  local win, err = M.resolve_view(spec)
  if not win then return nil, err end
  return vim.api.nvim_win_call(win, fn), nil
end

function M.state()
  local cur = vim.api.nvim_get_current_tabpage()
  local tabs = {}
  for _, tp in ipairs(vim.api.nvim_list_tabpages()) do
    local win = vim.api.nvim_tabpage_get_win(tp)
    local buf = vim.api.nvim_win_get_buf(win)
    tabs[#tabs + 1] = {
      tab = vim.api.nvim_tabpage_get_number(tp),
      view = tab_view[tp],
      win = win,
      current = tp == cur,
      buftype = vim.bo[buf].buftype,
      filetype = vim.bo[buf].filetype,
    }
  end
  return { current_view = tab_view[cur], tabs = tabs }
end

function M.last_view()
  local alt = M._alt
  if alt and vim.api.nvim_tabpage_is_valid(alt) and alt ~= vim.api.nvim_get_current_tabpage() then
    vim.api.nvim_set_current_tabpage(alt)
    core.restore_terminal_focus()
  end
end

return M
