-- Basic vim options and settings

-- Leader key (must be set before plugins)
vim.g.mapleader = " "

-- Persist history across sessions
vim.o.shada = "'1000,<50,s10,h,/100,:1000"
-- '1000 = Save marks for up to 1000 files
-- <50   = Save up to 50 lines for each register
-- s10   = Save items up to 10KB in size
-- h     = Disable hlsearch when loading the shada file
-- /100  = Save last 100 search patterns
-- :1000 = Save last 1000 command line entries

-- Additional persistence settings
vim.o.undofile = true  -- Enable persistent undo
vim.o.undolevels = 1000  -- Maximum number of changes that can be undone
vim.o.undoreload = 10000  -- Maximum number lines to save for undo on a buffer reload

-- Set directories for persistent data
local data_dir = vim.fn.stdpath("data")
vim.o.undodir = data_dir .. "/undo//"  -- Undo files location (// means use full path)
vim.o.backupdir = data_dir .. "/backup//"  -- Backup files location

-- Disable swap files for better performance (not needed with git and auto-save)
vim.o.swapfile = false

-- Create directories if they don't exist
for _, dir in ipairs({vim.o.undodir, vim.o.backupdir}) do
  if vim.fn.isdirectory(dir) == 0 then
    vim.fn.mkdir(dir, "p")
  end
end

-- Clipboard: Use OSC 52 (Ghostty/Kitty terminal clipboard protocol)
-- This works over SSH and doesn't require system clipboard access
-- vim.opt.clipboard = "unnamedplus"  -- Old: system clipboard via pbcopy/xclip
vim.g.clipboard = {
  name = 'OSC 52',
  copy = {
    ['+'] = require('vim.ui.clipboard.osc52').copy('+'),
    ['*'] = require('vim.ui.clipboard.osc52').copy('*'),
  },
  paste = {
    ['+'] = require('vim.ui.clipboard.osc52').paste('+'),
    ['*'] = require('vim.ui.clipboard.osc52').paste('*'),
  },
}
vim.opt.clipboard = "unnamedplus"  -- Use + register as default
vim.opt.scrolloff = 8  -- Keep 8 lines visible above/below cursor

-- Disable horizontal scrolling with mouse wheel
vim.opt.mousescroll = "ver:1,hor:0"  -- 1 line vertical scroll, 0 for horizontal (disabled)

-- Performance settings
vim.opt.updatetime = 50  -- Faster CursorHold events (default 4000ms) - helps multicursor
vim.opt.timeoutlen = 300  -- Faster key sequence completion
vim.opt.redrawtime = 10000  -- Allow 10s for syntax/treesitter highlighting (default 2000ms)

-- Additional settings
vim.opt.cmdheight = 0  -- Hide command line when not in use

-- Footer: single global statusline (one bar at the very bottom, not per-split)
vim.opt.laststatus = 3
-- Show the absolute, normalized, symlink-resolved path of the current buffer.
-- fnamemodify(':p') makes it absolute; fs_realpath follows symlinks so editing
-- a linked file (e.g. ~/.claude/CLAUDE.md -> the nix store) shows the real
-- target instead of the link; vim.fs.normalize tidies ./.. and ~. Empty name
-- for unnamed/special buffers.
function _G.statusline_path()
  local name = vim.api.nvim_buf_get_name(0)
  if name == "" then
    return "[No Name]"
  end
  local abs = vim.fn.fnamemodify(name, ":p")
  local real = (vim.uv or vim.loop).fs_realpath(abs) or abs
  return vim.fs.normalize(real)
end
vim.opt.statusline = "%{v:lua.statusline_path()}%m%=%l:%c"
vim.opt.number = true
vim.opt.relativenumber = true
vim.opt.numberwidth = 1  -- Auto-size number column to minimum needed
vim.opt.signcolumn = "yes"  -- Always show sign column for git signs
vim.opt.expandtab = true
vim.opt.shiftwidth = 4
vim.opt.tabstop = 4
vim.opt.smartindent = true
vim.opt.wrap = true  -- Enable soft wrap globally
vim.opt.linebreak = true  -- Break at word boundaries (not mid-word)
vim.opt.breakindent = true  -- Wrapped lines preserve indentation
vim.opt.showbreak = "↪ "  -- Visual indicator for wrapped lines
vim.opt.termguicolors = true
vim.opt.wrapscan = false

-- Terminal settings for better color support
vim.env.TERM = "xterm-256color"  -- Override TERM for better color support

-- Spell check settings
vim.opt.spell = false  -- Start with spell check disabled
vim.opt.spelllang = "en_us"  -- Set spell check language
vim.opt.spellfile = vim.fn.expand("~/.config/nvim/spell/en.utf-8.add")  -- Personal dictionary

-- Folding settings (using nvim-ufo with LSP)
vim.opt.foldcolumn = "1"  -- Show fold column
vim.opt.foldlevel = 99  -- Large value needed for nvim-ufo
vim.opt.foldlevelstart = 99  -- Start with most folds open (ufo handles closing)
vim.opt.foldenable = true  -- Enable folding by default

-- Auto-reload files changed outside vim
vim.opt.autoread = true
