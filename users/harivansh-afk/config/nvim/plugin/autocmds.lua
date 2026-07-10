local api = vim.api
local augroup = api.nvim_create_augroup("UserAutocmds", { clear = true })

local function ensure_canola_loaded()
  local canola_config = require "config.canola"
  canola_config.setup_globals()

  local ok_lz, lz = pcall(require, "lz.n")
  if ok_lz then pcall(lz.trigger_load, "barrettruth/canola.nvim") end

  if vim.fn.exists ":Canola" ~= 2 then pcall(vim.cmd.packadd, "canola.nvim") end

  canola_config.setup_integrations()
end

local function maybe_load_canola(bufnr)
  local name = api.nvim_buf_get_name(bufnr)
  if name == "" or vim.bo[bufnr].filetype == "canola" or vim.fn.isdirectory(name) == 0 then return end

  ensure_canola_loaded()
  pcall(vim.cmd, "silent keepalt Canola " .. vim.fn.fnameescape(name))
end

api.nvim_create_autocmd("TextYankPost", {
  group = augroup,
  callback = function() vim.highlight.on_yank { higroup = "Visual", timeout = 200 } end,
})

api.nvim_create_autocmd("BufReadPost", {
  group = augroup,
  callback = function()
    if ({ gitcommit = true, gitrebase = true })[vim.bo.filetype] then return end
    local mark = api.nvim_buf_get_mark(0, '"')
    if mark[1] > 0 and mark[1] <= api.nvim_buf_line_count(0) then pcall(api.nvim_win_set_cursor, 0, mark) end
  end,
})

api.nvim_create_autocmd("BufEnter", {
  group = augroup,
  nested = true,
  callback = function(args)
    if vim.v.vim_did_enter == 1 then maybe_load_canola(args.buf) end
  end,
})

api.nvim_create_autocmd("VimEnter", {
  group = augroup,
  nested = true,
  callback = function() maybe_load_canola(0) end,
})

api.nvim_create_autocmd("VimResized", {
  group = augroup,
  callback = function()
    local tab = vim.fn.tabpagenr()
    vim.cmd "tabdo wincmd ="
    vim.cmd("tabnext " .. tab)
  end,
})

-- Terminals open ready to type, and panes you were typing in resume
-- terminal-mode when you navigate back. <Esc> is never mapped: it always
-- reaches the program. Leaving terminal-mode and the window in one action
-- (prefix pane-nav) keeps the insert intent via b:term_leaving; an explicit
-- copy-mode entry (<c-b>[) sticks until you re-enter it (i, a, or G).
-- Programmatic leaves (mux view switching, via b:term_programmatic) also
-- keep the intent. Leaving with intent parks the cursor on the last line so
-- background panes tail their output.
api.nvim_create_autocmd("TermOpen", {
  group = augroup,
  callback = function(args)
    vim.b[args.buf].term_insert = true
    vim.keymap.set("n", "G", "Gi", { buffer = args.buf, desc = "jump to tail, resume terminal" })
    vim.cmd.startinsert()
  end,
})

api.nvim_create_autocmd("TermEnter", {
  group = augroup,
  callback = function() vim.b.term_insert = true end,
})

api.nvim_create_autocmd("TermLeave", {
  group = augroup,
  callback = function(args)
    local buf = args.buf
    if vim.b[buf].term_programmatic then
      vim.b[buf].term_programmatic = nil
      return
    end
    vim.b[buf].term_insert = false
    vim.b[buf].term_leaving = true
    vim.schedule(function()
      if api.nvim_buf_is_valid(buf) then vim.b[buf].term_leaving = false end
    end)
  end,
})

api.nvim_create_autocmd("WinLeave", {
  group = augroup,
  callback = function()
    if vim.bo.buftype ~= "terminal" then return end
    if vim.b.term_leaving then vim.b.term_insert = true end
    if vim.b.term_insert then pcall(api.nvim_win_set_cursor, 0, { api.nvim_buf_line_count(0), 0 }) end
  end,
})

api.nvim_create_autocmd("WinEnter", {
  group = augroup,
  callback = function()
    if vim.bo.buftype ~= "terminal" or not vim.b.term_insert then return end
    vim.schedule(function()
      if vim.bo.buftype == "terminal" and vim.b.term_insert then pcall(vim.cmd.startinsert) end
    end)
  end,
})
