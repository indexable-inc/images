local core = require "mux.core"
local engine = require "mux.view"

local canon = core.canon
local tab_view = core.tab_view
local views = core.views
local find_view = core.find_view
local sessions_dir = core.sessions_dir

local M = {}

---@param root string project root path
local function push_history(root)
  root = canon(root)
  if root == "" then return end
  local dir = core.state_dir()
  pcall(vim.fn.mkdir, dir, "p")
  local hf = dir .. "/history"
  local kept = {}
  if vim.fn.filereadable(hf) == 1 then
    for _, line in ipairs(vim.fn.readfile(hf)) do
      if line ~= "" and canon(line) ~= root then kept[#kept + 1] = line end
    end
  end
  kept[#kept + 1] = root
  while #kept > 50 do
    table.remove(kept, 1)
  end
  pcall(vim.fn.writefile, kept, hf)
end

---@param root string project root path
function M.record_last(root)
  if not root or root == "" then return end
  root = canon(root)
  local dir = core.state_dir()
  pcall(vim.fn.mkdir, dir, "p")
  push_history(root)
  pcall(vim.fn.writefile, { root }, dir .. "/last")
end

---@param root string project root path
local function forget_history(root)
  root = canon(root)
  if root == "" then return end
  local hf = core.state_dir() .. "/history"
  if vim.fn.filereadable(hf) ~= 1 then return end
  local kept = {}
  for _, line in ipairs(vim.fn.readfile(hf)) do
    if line ~= "" and canon(line) ~= root then kept[#kept + 1] = line end
  end
  pcall(vim.fn.writefile, kept, hf)
end

---@param root string project root path
local function clear_last(root)
  root = canon(root)
  local lf = core.state_dir() .. "/last"
  if vim.fn.filereadable(lf) ~= 1 then return end
  local cur = vim.fn.readfile(lf)[1]
  if cur and canon(cur) == root then pcall(vim.fn.delete, lf) end
end

---@return string
function M.root()
  local env = vim.env.MUX_ROOT
  if env and env ~= "" then return canon(env) end
  local root = vim.fs.root(vim.fn.getcwd(), { ".git", ".jj" })
  if root and root ~= "" then return canon(root) end
  return canon(vim.fn.getcwd())
end

---@return string
local function session_file()
  local env = vim.env.MUX_SESSION_FILE
  if env and env ~= "" then return env end
  local slug = M.root():gsub("[^%w._-]", "_")
  return sessions_dir() .. "/" .. slug .. ".vim"
end

---@return string
local function root_file() return (session_file():gsub("%.vim$", ".root")) end

local function restore_file() return (session_file():gsub("%.vim$", ".restore")) end

local function mark_restore()
  pcall(vim.fn.mkdir, vim.fn.fnamemodify(restore_file(), ":h"), "p")
  pcall(vim.fn.writefile, { M.root() }, restore_file())
end

local function unmark_restore() pcall(vim.fn.delete, restore_file()) end

---@param drop_extra boolean
---@return table<integer, boolean>
local function repair_views(drop_extra)
  local seen = {}
  local drop = {}
  for tp, view in pairs(tab_view) do
    if not vim.api.nvim_tabpage_is_valid(tp) or not views[view] or seen[view] then
      tab_view[tp] = nil
    else
      seen[view] = true
    end
  end
  local have_edit = seen.edit == true
  for _, tp in ipairs(vim.api.nvim_list_tabpages()) do
    if not tab_view[tp] then
      if not have_edit then
        tab_view[tp] = "edit"
        have_edit = true
      elseif drop_extra then
        drop[tp] = true
      end
    end
  end
  return drop
end

-- Soft stop: write all buffers and quit, leaving the saved session
-- so a next attach may resume the layout.
function M.stop_session()
  unmark_restore()
  pcall(vim.cmd, "silent! wall")
  vim.schedule(function() pcall(vim.cmd, "qall!") end)
end

-- Hard kill: delete the saved session and history/last entries, then quit.
function M.kill_session()
  M._killing = true
  local f = session_file()
  pcall(vim.fn.delete, f)
  pcall(vim.fn.delete, root_file())
  unmark_restore()
  local root = M.root()
  forget_history(root)
  clear_last(root)
  vim.schedule(function() pcall(vim.cmd, "qall!") end)
end

-- Reload in place: save the layout, then `:restart! +qall!` re-execs the server
-- with new config and reattaches the UI (setup restores the tabs).
function M.reload()
  if #vim.api.nvim_list_uis() == 0 then
    return -- no UI to reattach; :restart would dangle the new server
  end
  M.save_session()
  pcall(vim.cmd, "silent! wall")
  pcall(vim.cmd, "restart! +qall!")
end

function M.reload_all()
  vim.system({ "mux", "reload", "--all" }, function(res)
    if res.code ~= 0 then vim.schedule(function() vim.notify("mux reload --all failed", vim.log.levels.ERROR) end) end
  end)
end

function M.save_session()
  if M._killing then return end
  repair_views(false)
  local map = {}
  for tp, view in pairs(tab_view) do
    if vim.api.nvim_tabpage_is_valid(tp) then map[tostring(vim.api.nvim_tabpage_get_number(tp))] = view end
  end
  vim.g.MuxViews = vim.json.encode(map)
  local f = session_file()
  vim.fn.mkdir(vim.fn.fnamemodify(f, ":h"), "p")
  pcall(vim.cmd, "mksession! " .. vim.fn.fnameescape(f))
  pcall(vim.fn.writefile, { M.root() }, root_file())
end

function M.record_root()
  local f = session_file()
  pcall(vim.fn.mkdir, vim.fn.fnamemodify(f, ":h"), "p")
  pcall(vim.fn.writefile, { M.root() }, root_file())
  mark_restore()
end

---@return string?
local function pid_file()
  local sock = vim.v.servername
  if sock and sock:match "%.sock$" then return (sock:gsub("%.sock$", ".pid")) end
  return nil
end

function M.record_pid()
  local f = pid_file()
  if not f then return end
  pcall(vim.fn.mkdir, vim.fn.fnamemodify(f, ":h"), "p")
  pcall(vim.fn.writefile, { tostring(vim.fn.getpid()) }, f)
end

function M.clear_pid()
  local f = pid_file()
  if f then pcall(vim.fn.delete, f) end
end

---@return boolean restored
function M.load_session()
  if vim.fn.argc(-1) ~= 0 then return false end
  local f = session_file()
  if vim.fn.filereadable(f) == 0 then return false end
  if not pcall(vim.cmd, "silent! source " .. vim.fn.fnameescape(f)) then return false end
  for k in pairs(tab_view) do
    tab_view[k] = nil
  end
  local raw = vim.g.MuxViews
  if type(raw) == "string" and raw ~= "" then
    local okj, decoded = pcall(vim.json.decode, raw)
    if okj and type(decoded) == "table" then
      local tabs = vim.api.nvim_list_tabpages()
      for nr, view in pairs(decoded) do
        local tp = tabs[tonumber(nr)]
        if tp and vim.api.nvim_tabpage_is_valid(tp) then tab_view[tp] = view end
      end
    end
  end

  local cur = vim.api.nvim_get_current_tabpage()
  local drop = repair_views(true)
  for tp, view in pairs(tab_view) do
    local spec = views[view]
    if vim.api.nvim_tabpage_is_valid(tp) and spec then
      if spec.restore then
        vim.api.nvim_set_current_tabpage(tp)
        engine.materialize(view, true)
      elseif spec.kind ~= "editor" then
        drop[tp] = true
      end
    end
  end
  if drop[cur] then cur = find_view "edit" or cur end
  if vim.api.nvim_tabpage_is_valid(cur) then vim.api.nvim_set_current_tabpage(cur) end
  if vim.bo.buftype ~= "terminal" then pcall(vim.cmd, "stopinsert") end
  for tp in pairs(drop) do
    engine.close_view_tab(tp, true)
  end
  return true
end

return M
