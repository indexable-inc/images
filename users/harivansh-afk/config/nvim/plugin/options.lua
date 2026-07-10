local o, opt = vim.o, vim.opt

o.number = true
o.relativenumber = true

o.tabstop = 2
o.shiftwidth = 2
o.expandtab = true
o.smartindent = true
o.breakindent = true

o.ignorecase = true
o.smartcase = true
o.hlsearch = true
o.incsearch = true

o.termguicolors = true
o.scrolloff = 8
o.signcolumn = "yes"
o.wrap = false
o.showmode = false
o.laststatus = 3
o.cmdheight = 0

opt.fillchars = { vert = "|", fold = "-", foldsep = "|", diff = "-" }
opt.shortmess:append "S"

o.splitbelow = true
o.splitright = true

o.swapfile = false
o.backup = false
o.undofile = true
o.undodir = vim.fn.stdpath "data" .. "/undo"

o.foldlevel = 99
o.foldlevelstart = 99
o.foldenable = true

o.updatetime = 250
o.mouse = "a"
o.clipboard = "unnamedplus"

-- Clipboard on hosts without a local clipboard tool (e.g. headless mux
-- servers on spark): emit OSC 52 through nvim_ui_send, which forwards the
-- sequence to the attached --remote-ui client's terminal. Always use the
-- `c` selector: mosh-server 1.4 silently drops every other one. Paste
-- returns the local yank cache instead of querying the terminal, since
-- OSC 52 reads do not survive mosh; paste the OS clipboard with cmd-v.
local has_clip_tool = vim.fn.has "mac" == 1
  or (vim.env.WAYLAND_DISPLAY and vim.fn.executable "wl-copy" == 1)
  or (vim.env.DISPLAY and (vim.fn.executable "xclip" == 1 or vim.fn.executable "xsel" == 1))
if not has_clip_tool then
  local cache = { ["+"] = {}, ["*"] = {} }
  local function copy(reg)
    return function(lines)
      cache[reg] = lines
      local seq = ("\027]52;c;%s\027\\"):format(vim.base64.encode(table.concat(lines, "\n")))
      pcall(vim.api.nvim_ui_send, seq)
    end
  end
  local function paste(reg)
    return function() return cache[reg] end
  end
  vim.g.clipboard = {
    name = "osc52-write",
    copy = { ["+"] = copy "+", ["*"] = copy "*" },
    paste = { ["+"] = paste "+", ["*"] = paste "*" },
  }
end

require("statusline").setup()
