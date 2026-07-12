-- Bootstrap lazy.nvim
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not vim.loop.fs_stat(lazypath) then
  vim.fn.system({
    "git",
    "clone",
    "--filter=blob:none",
    "https://github.com/folke/lazy.nvim.git",
    "--branch=stable",
    lazypath,
  })
end
vim.opt.rtp:prepend(lazypath)

-- Load configuration modules
require("config.options")
require("config.autocmds")

-- Setup lazy.nvim with plugins from the plugins directory
require("lazy").setup("plugins", {
  -- Lazy.nvim configuration options
  ui = {
    border = "rounded",
  },
  change_detection = {
    notify = false,
  },
})

-- Load keymaps after plugins are loaded
require("config.keymaps")