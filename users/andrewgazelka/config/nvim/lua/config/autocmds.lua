-- Autocommands configuration

-- Auto-reload files when changed externally (instant via file watcher)
local file_watchers = {}

local function watch_file(bufnr)
  local filepath = vim.api.nvim_buf_get_name(bufnr)
  if filepath == '' or file_watchers[bufnr] then return end

  local handle = vim.uv.new_fs_event()
  if not handle then return end

  file_watchers[bufnr] = handle
  handle:start(filepath, {}, vim.schedule_wrap(function(err, _, _)
    if err then return end
    if not vim.api.nvim_buf_is_valid(bufnr) then
      handle:stop()
      file_watchers[bufnr] = nil
      return
    end
    -- Only reload if buffer is not modified
    if not vim.bo[bufnr].modified then
      vim.api.nvim_buf_call(bufnr, function()
        vim.cmd('checktime')
      end)
    end
  end))
end

local function stop_watch(bufnr)
  if file_watchers[bufnr] then
    file_watchers[bufnr]:stop()
    file_watchers[bufnr] = nil
  end
end

vim.api.nvim_create_autocmd("BufEnter", {
  pattern = "*",
  callback = function(ev)
    watch_file(ev.buf)
  end,
})

vim.api.nvim_create_autocmd("BufDelete", {
  pattern = "*",
  callback = function(ev)
    stop_watch(ev.buf)
  end,
})

-- Custom filetype detection
vim.filetype.add({
  extension = {
    bootskill = "json",
    env = "dotenv",
  },
  filename = {
    [".env"] = "dotenv",
    [".envrc"] = "bash",  -- direnv uses bash syntax
  },
  pattern = {
    -- Match .env.* files (e.g., .env.local, .env.production, .env.development)
    ["%.env%..*"] = "dotenv",
    -- Match .env files with optional suffixes
    [".*%.env"] = "dotenv",
  },
})

-- Auto-create directories when saving files (optimized with early exit)
vim.api.nvim_create_autocmd({"BufWritePre"}, {
  pattern = {"*"},
  callback = function()
    -- Skip for special buffers (terminals, help, etc.)
    if vim.bo.buftype ~= "" then
      return
    end

    local dir = vim.fn.expand('%:p:h')
    -- Only check if directory doesn't exist (fast path for most saves)
    if dir ~= "" and vim.fn.isdirectory(dir) == 0 then
      vim.fn.mkdir(dir, 'p')
    end
  end,
})

-- Restore cursor position when opening files (except git commits)
vim.api.nvim_create_autocmd({"BufReadPost"}, {
  pattern = {"*"},
  callback = function()
    local ft = vim.bo.filetype
    local filename = vim.fn.expand('%:t')
    if ft == "gitcommit" or filename == "COMMIT_EDITMSG" then
      return
    end
    if vim.fn.line("'\"") > 1 and vim.fn.line("'\"") <= vim.fn.line("$") then
      vim.cmd('normal! g`"')
    end
  end,
})


-- Enable list continuation in markdown files
vim.api.nvim_create_autocmd({"FileType"}, {
  pattern = {"markdown"},
  callback = function()
    vim.opt_local.formatoptions:append("r")  -- Continue lists on Enter
    vim.opt_local.comments = "b:-,b:*,b:+,b:>"  -- Define list markers and blockquotes
    -- Disable inlay hints for markdown
    vim.lsp.inlay_hint.enable(false, { bufnr = 0 })
    -- Enable soft wrap on word boundaries
    vim.opt_local.wrap = true
    vim.opt_local.linebreak = true
    vim.opt_local.breakindent = true  -- Preserve indentation on wrapped lines
    -- Disable line numbers
    vim.opt_local.number = false
    vim.opt_local.relativenumber = false
    -- Keep the footer visible in markdown too (laststatus is global; the
    -- footer shows the absolute path set in options.lua).
    vim.opt_local.ruler = false
    vim.opt_local.conceallevel = 0
    vim.opt_local.concealcursor = ""
    vim.opt_local.winhighlight = table.concat({
      "Normal:MarkdownNormal",
      "NormalNC:MarkdownNormal",
      "EndOfBuffer:MarkdownEndOfBuffer",
    }, ",")
    require("config.highlights").apply_markdown_notion()

    -- Markdown formatting keybindings
    local opts = { buffer = true, noremap = true, silent = true }
    -- Cmd+i for italic (wraps selection in * or adds * around word)
    vim.keymap.set('v', '<D-i>', 'c*<C-r>"*<Esc>', opts)
    vim.keymap.set('n', '<D-i>', 'viw<Esc>a*<Esc>bi*<Esc>', opts)
    vim.keymap.set('i', '<D-i>', '**<Left>', opts)
    -- Cmd+b for bold (wraps selection in ** or adds ** around word)
    vim.keymap.set('v', '<D-b>', 'c**<C-r>"**<Esc>', opts)
    vim.keymap.set('n', '<D-b>', 'viw<Esc>a**<Esc>2bi**<Esc>', opts)
    vim.keymap.set('i', '<D-b>', '****<Left><Left>', opts)
  end,
})

-- Dotenv file settings
vim.api.nvim_create_autocmd({"FileType"}, {
  pattern = {"dotenv"},
  callback = function()
    -- Disable spell check (env vars aren't real words)
    vim.opt_local.spell = false
    -- Disable inlay hints
    vim.lsp.inlay_hint.enable(false, { bufnr = 0 })
    -- Set comment string for .env files
    vim.opt_local.commentstring = "# %s"
  end,
})

-- Enable proper syntax highlighting for git commit messages
vim.api.nvim_create_autocmd({"FileType"}, {
  pattern = {"gitcommit"},
  callback = function()
    -- Enable syntax highlighting
    vim.cmd('syntax enable')
    -- Treesitter highlighting is auto-enabled via the nvim-treesitter plugin config
    -- No need to call TSEnable (old API) - just ensure gitcommit parser is installed
    -- Disable the vertical guide lines
    vim.opt_local.colorcolumn = ""
  end,
})

-- Ensure diff highlighting in git verbose commits
vim.api.nvim_create_autocmd({"BufRead", "BufNewFile"}, {
  pattern = {"COMMIT_EDITMSG"},
  callback = function()
    vim.cmd('set filetype=gitcommit')
  end,
})

-- Auto-apply transparent background after any colorscheme change
vim.api.nvim_create_autocmd("ColorScheme", {
  pattern = "*",
  callback = function()
    require("config.highlights").apply_all()
  end,
})
