-- Reasonable Neovim defaults shipped by the ix base profile.
-- Spliced into init.vim via programs.neovim.configure.customRC.
-- Generic only: no plugins, no terminal-specific clipboard wiring,
-- no statusline opinions.

-- Space leader is the widespread modern convention; must be set
-- before any plugin or keymap reads g.mapleader.
vim.g.mapleader = " "
vim.g.maplocalleader = " "

-- Numbers and signs. relativenumber gives a quick jump count;
-- signcolumn pinned to "yes" stops the column flickering in/out when
-- LSP or git signs appear.
vim.opt.number = true
vim.opt.relativenumber = true
vim.opt.signcolumn = "yes"

-- Indentation: spaces, four-wide, smart. Override per-buffer with
-- an editorconfig file or a filetype autocmd.
vim.opt.expandtab = true
vim.opt.shiftwidth = 4
vim.opt.tabstop = 4
vim.opt.smartindent = true

-- Soft wrap with word boundaries and a visible continuation arrow,
-- so long lines stay legible without needing to scroll horizontally.
vim.opt.wrap = true
vim.opt.linebreak = true
vim.opt.breakindent = true
vim.opt.showbreak = "↪ "

-- Truecolor for any modern terminal.
vim.opt.termguicolors = true

-- Keep eight lines of context around the cursor so the next thing the
-- operator is reading is already on screen when they scroll into it.
vim.opt.scrolloff = 8

-- Faster CursorHold / which-key style key sequence completion. Defaults
-- (4000ms / 1000ms) feel sluggish over an SSH session.
vim.opt.updatetime = 50
vim.opt.timeoutlen = 300

-- Persist undo across sessions; large history because operators reopen
-- the same config files repeatedly across reconnects. Swap files off:
-- the dev VM is single-operator and persistent undo + git already cover
-- the usual recovery cases without the .swp clutter.
vim.opt.undofile = true
vim.opt.undolevels = 1000
vim.opt.undoreload = 10000
vim.opt.swapfile = false

-- Default colorscheme. ix-islands-{dark,light} ship to ~/.config/nvim/
-- colors/ via home.file, so both are available via :colorscheme. Dark
-- is the default because operator SSH sessions are dark terminals.
vim.cmd.colorscheme("ix-islands-dark")
