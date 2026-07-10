vim.g.mapleader = " "
vim.g.maplocalleader = ","

vim.opt.background = "dark"
do
  local orig_notify = vim.notify
  vim.notify = function(msg, level, opts)
    if type(msg) == "string" and msg:find("Did not detect DSR response", 1, true) then return end
    return orig_notify(msg, level, opts)
  end
end

local home = os.getenv "HOME" or ""
local local_bin = home .. "/.local/bin"
if not (os.getenv "PATH" or ""):find(local_bin, 1, true) then
  local new_path = local_bin .. ":" .. (os.getenv "PATH" or "")
  vim.env.PATH = new_path
  vim.uv.os_setenv("PATH", new_path)
end

function _G.map(mode, lhs, rhs, opts)
  vim.keymap.set(mode, lhs, rhs, vim.tbl_extend("keep", opts or {}, { silent = true }))
end

function _G.bmap(mode, lhs, rhs, opts) _G.map(mode, lhs, rhs, vim.tbl_extend("force", opts or {}, { buffer = 0 })) end

local disabled_plugins = {
  "2html_plugin",
  "bugreport",
  "getscript",
  "getscriptPlugin",
  "gzip",
  "logipat",
  "matchit",
  "netrw",
  "netrwFileHandlers",
  "netrwPlugin",
  "netrwSettings",
  "optwin",
  "rplugin",
  "rrhelper",
  "synmenu",
  "tar",
  "tarPlugin",
  "tohtml",
  "tutor",
  "vimball",
  "vimballPlugin",
  "zip",
  "zipPlugin",
}

for _, plugin in ipairs(disabled_plugins) do
  vim.g["loaded_" .. plugin] = 1
end

vim.api.nvim_create_autocmd("BufEnter", {
  group = vim.api.nvim_create_augroup("auto_project_root", { clear = true }),
  callback = function(args)
    if vim.bo[args.buf].buftype ~= "" then return end
    local name = vim.api.nvim_buf_get_name(args.buf)
    if name == "" or name:find "://" then return end
    if not vim.uv.fs_stat(name) then return end
    local root = vim.fs.root(name, { ".git", "flake.nix", "package.json", "Cargo.toml", "pyproject.toml" })
    if root and root ~= vim.fn.getcwd(-1, 0) then vim.cmd.lcd(root) end
  end,
})

vim.g.lz_n = {
  load = function(name) vim.cmd.packadd(name:match "[^/]+$" or name) end,
}

vim.pack.add {
  "https://github.com/lumen-oss/lz.n",
}

require("lz.n").load "plugins"
