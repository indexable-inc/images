local M = {}

local mode_labels = {
  n = "NORMAL",
  no = "OP",
  nov = "OP",
  noV = "OP",
  ["no\22"] = "OP",
  niI = "NORMAL",
  niR = "NORMAL",
  niV = "NORMAL",
  nt = "NORMAL",
  ntT = "NORMAL",
  v = "VISUAL",
  vs = "VISUAL",
  V = "V-LINE",
  Vs = "V-LINE",
  ["\22"] = "V-BLOCK",
  ["\22s"] = "V-BLOCK",
  s = "SELECT",
  S = "S-LINE",
  ["\19"] = "S-BLOCK",
  i = "INSERT",
  ic = "INSERT",
  ix = "INSERT",
  R = "REPLACE",
  Rc = "REPLACE",
  Rx = "REPLACE",
  Rv = "V-REPLACE",
  Rvc = "V-REPLACE",
  Rvx = "V-REPLACE",
  c = "COMMAND",
  cv = "EX",
  r = "PROMPT",
  rm = "MORE",
  ["r?"] = "CONFIRM",
  ["!"] = "SHELL",
  t = "TERMINAL",
}

local function segment(value)
  if value == nil or value == "" then return "" end
  return " " .. value .. " "
end

local function diff_segment(group, value)
  if value == nil or value == "" then return "" end
  return ("%%#%s#%s%%#StatusLine#"):format(group, value)
end

local function mode() return mode_labels[vim.api.nvim_get_mode().mode] or "NORMAL" end

local function terminal_label(name)
  local command = name:match "//%d+:(.+)$" or name:match "term://.-//(.+)$"
  if command == nil or command == "" then return "term" end

  command = command:gsub("^%s+", ""):match "^%S+" or command
  local label = vim.fn.fnamemodify(command, ":t")
  if label == "" then return "term" end

  return label
end

local function path()
  local name = vim.api.nvim_buf_get_name(0)
  if vim.bo.buftype == "terminal" then return terminal_label(name) end
  if name == "" then return "[No Name]" end
  return vim.fn.fnamemodify(name, ":~:.")
end

local function flags()
  local parts = {}

  if vim.bo.modified then table.insert(parts, "+") end
  if vim.bo.readonly or not vim.bo.modifiable then table.insert(parts, "") end

  return table.concat(parts, " ")
end

local function git_branch()
  if vim.fn.exists "*FugitiveHead" == 0 then return "" end

  local ok, head = pcall(vim.fn.FugitiveHead)
  if not ok or head == "" then return "" end

  return head
end

local function git_diff()
  local status = vim.b.gitsigns_status_dict
  if not status then return "" end

  local parts = {}
  if status.added and status.added > 0 then table.insert(parts, diff_segment("GitSignsAdd", "+" .. status.added)) end
  if status.changed and status.changed > 0 then
    table.insert(parts, diff_segment("GitSignsChange", "!" .. status.changed))
  end
  if status.removed and status.removed > 0 then
    table.insert(parts, diff_segment("GitSignsDelete", "-" .. status.removed))
  end

  return table.concat(parts, " ")
end

local function search_count()
  if vim.v.hlsearch == 0 then return "" end

  local ok, count = pcall(vim.fn.searchcount, { recompute = true, maxcount = 999 })
  if not ok or type(count) ~= "table" or (count.total or 0) == 0 then return "" end

  return ("%d/%d"):format(count.current or 0, count.total)
end

local function canola_path()
  if vim.bo.filetype ~= "canola" then return "" end

  local ok, canola = pcall(require, "canola")
  if not ok then return "canola" end

  local dir = canola.get_current_dir(0)
  if not dir then return "canola" end

  return vim.fn.fnamemodify(dir:gsub("/$", ""), ":~:.")
end

local function git_buffer_label()
  if not ({ fugitive = true, git = true, gitcommit = true, gitrebase = true })[vim.bo.filetype] then return "" end

  local branch = git_branch()
  if branch == "" then return "git" end

  return "git:" .. branch
end

local function progress()
  local line = vim.fn.line "."
  local total = vim.fn.line "$"
  if line <= 1 then return "top" end
  if line >= total then return "bot" end

  return ("%d%%%%"):format(math.floor(line / total * 100))
end

function M.render()
  local buffer_label = canola_path()
  if buffer_label == "" then buffer_label = git_buffer_label() end
  if buffer_label == "" then buffer_label = path() end

  local left = {
    segment(mode()),
    segment(git_branch()),
    segment(git_diff()),
    segment(buffer_label),
    segment(flags()),
  }

  local right = {
    segment(search_count()),
    segment(progress()),
  }

  return table.concat(left, "") .. "%=" .. table.concat(right, "")
end

function M.setup()
  vim.o.statusline = "%!v:lua.require'statusline'.render()"

  vim.api.nvim_create_autocmd("User", {
    group = vim.api.nvim_create_augroup("UserStatusline", { clear = true }),
    pattern = { "GitSignsUpdate", "GitSignsChanged" },
    callback = function() vim.cmd.redrawstatus() end,
  })
end

return M
