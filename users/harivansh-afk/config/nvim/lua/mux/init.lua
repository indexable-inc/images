-- mux: in-Neovim "multiplexer" brain. Each "view" is a tagged tabpage; untagged
-- tabpages are plain tmux-style windows. Switching projects is `:connect` to
-- another per-project nvim server (see scripts/bin/mux.sh). Bindings mirror the
-- old tmux config: <c-b> prefix, h/j/k/l panes, -/' splits, c new window,
-- [ copy mode, H/J/K/L session cycling, f local picker, F all-hosts picker,
-- d detach, \ tab bar toggle.

local core = require "mux.core"
local view = require "mux.view"
local project = require "mux.project"
local session = require "mux.session"
local line = require "mux.line"

local M = {}

M.views = core.views

M.open_view = view.open_view
M.resolve_view = view.resolve_view
M.in_view = view.in_view
M.state = view.state
M.last_view = view.last_view
M.close_view = view.close_view
M.kill_pane = view.kill_pane
M.new_window = view.new_window
M.split_terminal = view.split_terminal
M.toggle_zoom = view.toggle_zoom

M._connect = project._connect
M.pick_project = project.pick_project
M.pick_project_all = project.pick_project_all
M.cycle_project = project.cycle_project
M.list_entries = project.list_entries
M.last_session = project.last_session
M.stop_to_latest = project.stop_to_latest
M.kill_to_latest = project.kill_to_latest

M.stop_session = session.stop_session
M.kill_session = session.kill_session
M.reload = session.reload
M.reload_all = session.reload_all
M.save_session = session.save_session
M.load_session = session.load_session

local AUTOSAVE_INTERVAL_MS = 5 * 60 * 1000

function M.setup()
  if M._did then return end
  M._did = true

  vim.o.sessionoptions = "buffers,curdir,folds,globals,help,tabpages,winsize,winpos"
  vim.opt.fillchars:append { vert = "│" }

  local prefix = "<c-b>"
  local modes = { "n", "i", "t" }

  ---@param lhs string
  ---@param rhs fun()
  ---@param desc string
  local function muxmap(lhs, rhs, desc)
    local function wrapped()
      rhs()
      line.refresh()
    end
    vim.keymap.set(modes, lhs, wrapped, { desc = desc })
    -- Ctrl still held from the prefix: <c-b><c-n> must mean <c-b>n, not fall
    -- through to a raw window command (<c-w><c-n> opens an empty split).
    local key = lhs:sub(#prefix + 1)
    if key:match "^%l$" then vim.keymap.set(modes, prefix .. "<c-" .. key .. ">", wrapped, { desc = desc }) end
  end

  local function move_pane(primary, fallback)
    local win = vim.api.nvim_get_current_win()
    vim.cmd("wincmd " .. primary)
    if vim.api.nvim_get_current_win() == win then vim.cmd("wincmd " .. fallback) end
    core.restore_terminal_focus()
  end

  -- Prefix falls through to <c-w>; h/j/k/l get tmux-style edge fallback below
  -- and any unmapped <c-b>* keeps its window-command meaning. Every lowercase
  -- binding also answers its held-ctrl chord (see muxmap).
  for mode, rhs in pairs {
    n = "<c-w>",
    i = "<c-o><c-w>",
    t = "<c-\\><c-n><c-w>",
  } do
    vim.keymap.set(mode, prefix, rhs, { remap = true, desc = "mux: window command prefix" })
  end
  vim.keymap.set("t", prefix .. prefix, prefix, { desc = "mux: send prefix" })
  vim.keymap.set("t", prefix .. "[", [[<c-\><c-n>]], { desc = "mux: copy mode" })

  -- tmux parity
  muxmap(prefix .. "-", function() M.split_terminal(false) end, "mux: horizontal split terminal")
  muxmap(prefix .. "'", function() M.split_terminal(true) end, "mux: vertical split terminal")
  muxmap(prefix .. "h", function() move_pane("h", "l") end, "mux: pane left or right")
  muxmap(prefix .. "j", function() move_pane("j", "k") end, "mux: pane down or up")
  muxmap(prefix .. "k", function() move_pane("k", "j") end, "mux: pane up or down")
  muxmap(prefix .. "l", function() move_pane("l", "h") end, "mux: pane right or left")
  muxmap(prefix .. "c", M.new_window, "mux: new window")
  muxmap(prefix .. "x", M.kill_pane, "mux: kill pane")
  muxmap(prefix .. "z", M.toggle_zoom, "mux: zoom pane (toggle fullscreen)")
  muxmap(prefix .. "n", function() vim.cmd "tabnext" end, "mux: next window")
  muxmap(prefix .. "p", function() vim.cmd "tabprevious" end, "mux: previous window")
  muxmap(prefix .. "y", function()
    core.leave_terminal()
    pcall(vim.cmd, "buffer #")
  end, "mux: previous buffer (last shell)")
  for i = 1, 9 do
    muxmap(prefix .. i, function()
      local tabs = vim.api.nvim_list_tabpages()
      if tabs[i] then vim.api.nvim_set_current_tabpage(tabs[i]) end
    end, "mux: window " .. i)
  end
  for _, key in ipairs { "H", "K" } do
    muxmap(prefix .. key, function() M.cycle_project(-1) end, "mux: previous session")
  end
  for _, key in ipairs { "J", "L" } do
    muxmap(prefix .. key, function() M.cycle_project(1) end, "mux: next session")
  end
  muxmap(prefix .. "f", M.pick_project, "mux: switch project")
  muxmap(prefix .. "F", M.pick_project_all, "mux: switch project (all hosts)")
  muxmap(prefix .. "d", function() vim.cmd "detach" end, "mux: detach to shell")

  -- views + session lifecycle
  for name, spec in pairs(core.views) do
    local view_name = name
    muxmap(prefix .. spec.key, function() M.open_view(view_name) end, "mux: " .. view_name)
  end
  muxmap(prefix .. "<tab>", M.last_session, "mux: last session")
  muxmap(prefix .. "<bs>", M.last_session, "mux: last session")
  muxmap(prefix .. "6", M.last_view, "mux: last view")
  muxmap(prefix .. "s", M.save_session, "mux: save session")
  muxmap(prefix .. "S", M.stop_to_latest, "mux: stop session (hop to last)")
  muxmap(prefix .. "X", M.kill_to_latest, "mux: kill session (hop to last)")
  muxmap(prefix .. "R", M.reload, "mux: reload session (restart)")
  muxmap(prefix .. [[\]], line.toggle, "mux: toggle tab bar")

  local group = vim.api.nvim_create_augroup("mux", { clear = true })
  line.setup_hl()
  line.apply_visibility()
  vim.api.nvim_create_autocmd("ColorScheme", {
    group = group,
    callback = line.setup_hl,
  })
  vim.api.nvim_create_autocmd("TabClosed", {
    group = group,
    callback = function()
      core.prune()
      line.refresh()
    end,
  })
  vim.api.nvim_create_autocmd({ "TabNew", "DirChanged", "WinNew", "WinClosed", "WinResized" }, {
    group = group,
    callback = line.refresh,
  })
  -- OSC 7 from the shell (see zshrc _mux_osc7): track each terminal's live
  -- cwd so untagged windows rename like tmux automatic-rename.
  vim.api.nvim_create_autocmd("TermRequest", {
    group = group,
    callback = function(args)
      local seq = type(args.data) == "table" and args.data.sequence or args.data
      local dir = type(seq) == "string" and seq:match "^\027]7;file://[^/]*(/.*)$" or nil
      if not dir then return end
      dir = dir:gsub("%%(%x%x)", function(hex) return string.char(tonumber(hex, 16)) end)
      vim.b[args.buf].mux_term_cwd = dir
      line.refresh()
    end,
  })
  vim.api.nvim_create_autocmd("TabEnter", {
    group = group,
    callback = function()
      vim.schedule(core.restore_terminal_focus)
      line.refresh()
    end,
  })
  vim.api.nvim_create_autocmd("UIEnter", {
    group = group,
    callback = function()
      line.setup_hl()
      line.apply_visibility()
      line.refresh()
      vim.schedule(core.restore_terminal_focus)
    end,
  })

  -- tmux pane semantics: when a terminal's job exits, its pane goes with it,
  -- except in persistent task views (build/test) where the output stays.
  vim.api.nvim_create_autocmd("TermClose", {
    group = group,
    callback = function(args)
      if not vim.api.nvim_buf_is_valid(args.buf) then return end
      for _, win in ipairs(vim.fn.win_findbuf(args.buf)) do
        if vim.api.nvim_win_is_valid(win) and vim.api.nvim_win_get_config(win).relative == "" then
          local tp = vim.api.nvim_win_get_tabpage(win)
          local name = core.tab_view[tp]
          local spec = name and core.views[name]
          if not (spec and spec.lifecycle == "persistent") then view.on_terminal_exit(win, args.buf) end
        end
      end
    end,
  })
  vim.api.nvim_create_autocmd("TabLeave", {
    group = group,
    callback = function()
      view._alt = vim.api.nvim_get_current_tabpage()
      line.refresh()
    end,
  })
  vim.api.nvim_create_autocmd("VimLeavePre", {
    group = group,
    callback = function()
      line.stop_watchers()
      session.save_session()
      session.clear_pid()
    end,
  })

  session.record_root()
  session.record_pid()
  line.start_watchers()

  if not session.load_session() then
    view.materialize "zsh"
    core.tag(vim.api.nvim_get_current_tabpage(), "zsh")
  end
  line.apply_visibility()
  line.refresh()

  M._timer = vim.uv.new_timer()
  M._timer:start(AUTOSAVE_INTERVAL_MS, AUTOSAVE_INTERVAL_MS, vim.schedule_wrap(function() session.save_session() end))
end

return M
