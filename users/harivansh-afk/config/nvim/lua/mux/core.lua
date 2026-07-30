local M = {}

---@class mux.ViewSpec
---@field key string single char appended to the prefix to open this view
---@field kind 'editor'|'vcs'|'terminal' how the tab's content is built
---@field cmd? string[] terminal command (terminal kind)
---@field lifecycle? 'ephemeral'|'persistent' ephemeral tabs auto-close when their terminal exits
---@field restore? boolean re-materialize this view when a saved session is loaded

---@type table<string, mux.ViewSpec>
M.views = {
  edit = { key = "e", kind = "editor" },
  ai = {
    key = "a",
    kind = "terminal",
    cmd = { "omp" },
    lifecycle = "ephemeral",
    restore = true,
  },
  zsh = {
    key = "t",
    kind = "terminal",
    cmd = { vim.o.shell },
    lifecycle = "ephemeral",
    restore = true,
  },
  vcs = { key = "g", kind = "vcs", restore = true },
}

M.VIEW_ORDER = { "edit", "vcs", "ai", "zsh" }

-- tabpage handle -> view name
---@type table<integer, string>
M.tab_view = {}

---@param tabpage integer
---@param name string
function M.tag(tabpage, name) M.tab_view[tabpage] = name end

function M.prune()
  for tp in pairs(M.tab_view) do
    if not vim.api.nvim_tabpage_is_valid(tp) then M.tab_view[tp] = nil end
  end
end

---@param name string
---@return integer? tabpage handle of the open view, or nil if none
function M.find_view(name)
  for tp, v in pairs(M.tab_view) do
    if v == name and vim.api.nvim_tabpage_is_valid(tp) then return tp end
  end
  return nil
end

---@param tp integer
---@return integer count of non-floating windows in the tabpage
function M.live_window_count(tp)
  local n = 0
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(tp)) do
    if vim.api.nvim_win_get_config(w).relative == "" then n = n + 1 end
  end
  return n
end

---@param win integer
---@return boolean true if `win` is the last non-floating window in its tabpage
function M.last_window(win) return M.live_window_count(vim.api.nvim_win_get_tabpage(win)) <= 1 end

---@param p string?
---@return string
function M.canon(p)
  if not p or p == "" then return "" end
  local absolute = vim.fn.fnamemodify(p, ":p")
  if absolute == "/" then return absolute end
  return (absolute:gsub("/$", ""))
end

---@return string
function M.state_dir() return vim.fn.stdpath "state" .. "/mux" end

---@return string
function M.runtime_dir()
  local base = vim.env.XDG_RUNTIME_DIR
  if not base or base == "" then
    if vim.uv.os_uname().sysname == "Darwin" then
      base = (vim.env.TMPDIR or "/tmp"):gsub("/$", "")
    else
      base = "/run/user/" .. vim.uv.getuid()
    end
  end
  return base .. "/mux"
end

---@return string dir
function M.sessions_dir() return M.state_dir() .. "/sessions" end

-- Leave terminal-mode so a stray <c-c> hits Normal mode, not the terminal.
function M.leave_terminal()
  if vim.fn.mode() == "t" then
    vim.b.term_programmatic = true
    vim.cmd.stopinsert()
  end
end

---@return boolean
function M.restore_terminal_focus()
  local buf = vim.api.nvim_get_current_buf()
  if vim.bo[buf].buftype ~= "terminal" or not vim.b[buf].term_insert then return false end
  pcall(vim.cmd.startinsert)
  return true
end

---@param buf integer
---@return string?
function M.terminal_cwd(buf)
  if not vim.api.nvim_buf_is_valid(buf) or vim.bo[buf].buftype ~= "terminal" then return nil end
  local cwd = vim.b[buf].mux_term_cwd
  if cwd and cwd ~= "" and vim.fn.isdirectory(cwd) == 1 then return cwd end
  local job = vim.b[buf].terminal_job_id
  local pid = job and vim.fn.jobpid(job) or 0
  if not pid or pid <= 0 then return nil end
  local ok, dir = pcall(vim.uv.fs_readlink, "/proc/" .. pid .. "/cwd")
  if ok and dir and dir ~= "" and vim.fn.isdirectory(dir) == 1 then return dir end
  return nil
end

---@param msg string
function M.log(msg)
  local file = vim.env.MUX_LOG_FILE
  if not file or file == "" then return end
  pcall(vim.fn.mkdir, vim.fn.fnamemodify(file, ":h"), "p")
  pcall(vim.fn.writefile, { ("[%s] %s"):format(os.date "%Y-%m-%dT%H:%M:%S%z", msg) }, file, "a")
end

return M
