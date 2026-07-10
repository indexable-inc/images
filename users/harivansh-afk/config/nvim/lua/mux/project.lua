local core = require "mux.core"
local session = require "mux.session"

local views = core.views
local VIEW_ORDER = core.VIEW_ORDER
local canon = core.canon

local M = {}

local leave_terminal = core.leave_terminal
local sessions_dir = core.sessions_dir

local function load_fzf()
  local ok_lz, lz = pcall(require, "lz.n")
  if ok_lz then pcall(lz.trigger_load, "ibhagwan/fzf-lua") end
  if vim.fn.exists ":FzfLua" ~= 2 then pcall(vim.cmd.packadd, "fzf-lua") end
  return require "fzf-lua"
end

local function confirm_kill(root, cb)
  local name = vim.fn.fnamemodify(root, ":~")
  vim.schedule(function()
    vim.ui.input({ prompt = "mux: kill session " .. name .. "? [y/N]: " }, function(input)
      local answer = (input or ""):lower():match "^%s*(.-)%s*$"
      if answer == "y" or answer == "yes" then cb() end
    end)
  end)
end

---@param pid integer?
---@return boolean
local function pid_alive(pid)
  if not pid then return false end
  local ok, res = pcall(vim.uv.kill, pid, 0)
  return ok and res == 0
end

---@param sock string
---@return boolean
local function socket_live(sock)
  local pidf = (sock:gsub("%.sock$", ".pid"))
  if vim.fn.filereadable(pidf) == 1 then return pid_alive(tonumber(vim.fn.readfile(pidf)[1])) end
  local ok, ch = pcall(vim.fn.sockconnect, "pipe", sock, { rpc = true })
  if ok and type(ch) == "number" and ch > 0 then
    pcall(vim.fn.chanclose, ch)
    return true
  end
  return false
end

---@param slug string
---@return string? root
local function root_for_slug(slug)
  local f = sessions_dir() .. "/" .. slug .. ".root"
  if vim.fn.filereadable(f) == 1 then
    local root = vim.fn.readfile(f)[1]
    if root and root ~= "" then return root end
  end
  return nil
end

---@param path string?
---@return string?, string?
local function validate_dir(path)
  local root = canon(vim.trim(path or ""))
  if root == "" then return nil, "empty path" end
  if vim.fn.isdirectory(root) ~= 1 then return nil, ("not a directory: %s"):format(vim.fn.fnamemodify(root, ":~")) end
  return root, nil
end

---@param err string
local function notify_path_error(err)
  vim.notify("mux: " .. err, vim.log.levels.ERROR)
  core.restore_terminal_focus()
end

---@return { cwd: string, socket: string, status: string }[]
local function list_entries()
  local entries = {}
  local socks = vim.fn.glob(core.runtime_dir() .. "/*.sock", true, true)
  table.sort(socks)
  for _, sock in ipairs(socks) do
    if socket_live(sock) then
      local slug = vim.fn.fnamemodify(sock, ":t:r")
      local cwd = root_for_slug(slug)
      if cwd then entries[#entries + 1] = { cwd = cwd, socket = sock, status = "live" } end
    end
  end
  return entries
end

M.list_entries = list_entries

---@param status string
---@return boolean
local function known_project_status(status) return status == "live" or status == "dir" end

---@param output string?
---@return { cwd: string, socket: string, status: string }[]
local function parse_list_output(output)
  local entries = {}
  for line in (output or ""):gmatch "[^\n]+" do
    local cwd, socket, status = line:match "([^\t]*)\t([^\t]*)\t([^\t]*)"
    if cwd and cwd ~= "" and known_project_status(status) then
      entries[#entries + 1] = { cwd = cwd, socket = socket, status = status }
    end
  end
  return entries
end

---@param output string?
---@return { host: string, cwd: string, socket: string, status: string }[]
local function parse_list_all_output(output)
  local entries = {}
  for line in (output or ""):gmatch "[^\n]+" do
    local host, cwd, socket, status = line:match "([^\t]*)\t([^\t]*)\t([^\t]*)\t([^\t]*)"
    if host and host ~= "" and cwd and cwd ~= "" and known_project_status(status) then
      entries[#entries + 1] = { host = host, cwd = cwd, socket = socket, status = status }
    end
  end
  return entries
end

local function local_host_name()
  local h = vim.fn.hostname()
  h = (h:gsub("%..*$", ""))
  if h == "spark-ix" or h == "spark" then return "spark" end
  return h
end

---@param entry { host: string, path: string, socket: string?, status: string }
---@param view string?
local function connect_or_hop(entry, view)
  if not entry then return end
  local me = local_host_name()
  if entry.host == me then
    M._connect({ path = entry.path, socket = entry.socket }, view)
    return
  end
  leave_terminal()
  vim.system({ "mux", "hop", entry.host, entry.path }, { text = true }, function(res)
    vim.schedule(function()
      if res.code ~= 0 then
        vim.notify("mux: hop failed: " .. (res.stderr or res.stdout or ""), vim.log.levels.ERROR)
        core.restore_terminal_focus()
        return
      end
      vim.cmd "detach"
    end)
  end)
end

-- host tag colors: distinct cozybox hues, away from the status-tag lane
-- (green=live). Hosts are colored round-robin in
-- sorted order, so a given host set always colors the same way and no two
-- hosts share a hue until there are more hosts than palette entries.
local HOST_HL = { "CozyboxBlue", "CozyboxPurple", "CozyboxOrange", "CozyboxAqua" }

---@param entries { host: string }[]
---@return table<string, string>
local function host_colors(entries)
  local hosts, seen = {}, {}
  for _, e in ipairs(entries) do
    if not seen[e.host] then
      seen[e.host] = true
      hosts[#hosts + 1] = e.host
    end
  end
  table.sort(hosts)
  local map = {}
  for i, h in ipairs(hosts) do
    map[h] = HOST_HL[(i - 1) % #HOST_HL + 1]
  end
  return map
end

local STATUS_RANK = { live = 1, dir = 2 }

-- Build and show the project picker from `mux list`: live sessions (green
-- dot) first, then openable dirs, each with the ~-shortened path. Federated
-- rows carry a [host] tag in that host's stable color. Selecting connects
-- to that project.
---@param items { cwd: string, socket: string, status: string, host: string? }[]
---@param opts { federated: boolean? }?
local function show_picker(items, opts)
  opts = opts or {}
  local federated = opts.federated == true
  local me = local_host_name()
  ---@type { path: string, socket: string?, status: string, disp: string, host: string }[]
  local entries = {}
  local w = 0
  local hw = 0
  for _, item in ipairs(items or {}) do
    local cwd, sock, status = item.cwd, item.socket, item.status
    local host = item.host or me
    if cwd and cwd ~= "" and known_project_status(status) then
      local path, disp
      if host == me then
        path = canon(cwd)
        disp = vim.fn.fnamemodify(path, ":~")
      else
        path = cwd
        disp = (cwd:gsub("^/home/[^/]+", "~"):gsub("^/Users/[^/]+", "~"))
      end
      if #disp > w then w = #disp end
      if #host > hw then hw = #host end
      entries[#entries + 1] = {
        path = path,
        socket = (sock and sock ~= "" and sock) or nil,
        status = status,
        disp = disp,
        host = host,
      }
    end
  end

  for i, e in ipairs(entries) do
    e.rank = i
  end
  table.sort(entries, function(a, b)
    local ra, rb = STATUS_RANK[a.status], STATUS_RANK[b.status]
    if ra ~= rb then return ra < rb end
    return a.rank < b.rank
  end)

  if #entries == 0 then
    core.restore_terminal_focus()
    return
  end

  -- strip SGR codes so meta lookups work whether fzf hands back the colored
  -- row or a plain one.
  local function strip_ansi(s) return (s:gsub("\27%[[%d;]*m", "")) end
  local fzf = load_fzf()
  local hl = require("fzf-lua.utils").ansi_from_hl
  -- live rows get a colored dot; dirs a blank gutter (picker coloring only)
  local tag_hl = { live = "DiagnosticOk" }

  local host_hl = federated and host_colors(entries) or {}
  local color_lines, meta = {}, {}
  for _, e in ipairs(entries) do
    local tag = e.status == "live" and "● " or "  "
    local host_tag = federated and ("%-" .. (hw + 2) .. "s"):format(("[%s]"):format(e.host)) or ""
    local rest, line
    if federated then
      rest = ((" %s %-" .. w .. "s"):format(host_tag, e.disp):gsub("%s+$", ""))
      local colored_host = hl and host_hl[e.host] and hl(host_hl[e.host], host_tag) or host_tag
      line = ((" %s %-" .. w .. "s"):format(colored_host, e.disp):gsub("%s+$", ""))
    else
      rest = ((" %-" .. w .. "s  %s"):format(e.disp, e.socket or ""):gsub("%s+$", ""))
      line = rest
    end
    local group = tag_hl[e.status]
    color_lines[#color_lines + 1] = (hl and group and hl(group, tag) or tag) .. line
    meta[tag .. rest] = { path = e.path, socket = e.socket, status = e.status, host = e.host }
  end

  local actions = {
    ["default"] = function(sel)
      local entry = sel and sel[1] and meta[strip_ansi(sel[1])]
      if entry then connect_or_hop(entry) end
    end,
    ["ctrl-o"] = function(_, fzf_opts)
      if federated then return end
      local path, err = validate_dir(fzf_opts and fzf_opts.last_query or nil)
      if not path then
        notify_path_error(err or "invalid path")
        return
      end
      M._connect { path = path }
    end,
  }
  local parts = {}
  if not federated then
    for _, name in ipairs(VIEW_ORDER) do
      local spec = views[name]
      actions["ctrl-" .. spec.key] = function(sel)
        local entry = sel and sel[1] and meta[strip_ansi(sel[1])]
        if entry then connect_or_hop(entry, name) end
      end
      parts[#parts + 1] = ("%s %s"):format(
        hl("FzfLuaHeaderBind", "^" .. spec.key:upper()),
        hl("FzfLuaHeaderText", name)
      )
    end
    parts[#parts + 1] = ("%s %s"):format(hl("FzfLuaHeaderBind", "^O"), hl("FzfLuaHeaderText", "open"))
  end
  local function lifecycle(verb, sel)
    if federated then return end
    local entry = sel and sel[1] and meta[strip_ansi(sel[1])]
    if entry and entry.status ~= "live" then return end
    if entry and entry.path then
      if canon(entry.path) == session.root() then
        if verb == "kill" then
          confirm_kill(entry.path, session.kill_session)
        else
          session.stop_session()
        end
        return
      end
      local function run()
        vim.system({ "mux", verb, entry.path }, function() vim.schedule(M.pick_project) end)
      end
      if verb == "kill" then
        confirm_kill(entry.path, run)
      else
        run()
      end
      return
    end
    vim.schedule(M.pick_project)
  end
  if not federated then
    actions["ctrl-s"] = function(sel) lifecycle("stop", sel) end
    parts[#parts + 1] = ("%s %s"):format(hl("FzfLuaHeaderBind", "^S"), hl("FzfLuaHeaderText", "stop"))
    actions["ctrl-x"] = function(sel) lifecycle("kill", sel) end
    parts[#parts + 1] = ("%s %s"):format(hl("FzfLuaHeaderBind", "^X"), hl("FzfLuaHeaderText", "kill"))
  end
  fzf.fzf_exec(color_lines, {
    prompt = federated and "host-project> " or "project> ",
    fzf_args = (vim.env.FZF_DEFAULT_OPTS or ""):gsub("%-%-bind=ctrl%-a:select%-all", ""),
    keymap = { fzf = { ["ctrl-z"] = false } },
    fzf_opts = {
      ["--ansi"] = true,
      ["--header"] = #parts > 0 and (":: " .. table.concat(parts, " | ")) or false,
    },
    winopts = {
      on_close = function() vim.schedule(core.restore_terminal_focus) end,
    },
    actions = actions,
  })
end

---@param entry { path: string, socket: string? }
---@param view string?
function M._connect(entry, view)
  if not entry then return end
  if not entry.socket or entry.socket == "" then
    local path, err = validate_dir(entry.path)
    if not path then
      notify_path_error(err)
      return
    end
    entry.path = path
  end
  if entry.socket and entry.socket ~= "" and entry.socket == vim.v.servername then
    session.record_last(entry.path)
    if view then
      require("mux.view").open_view(view)
    else
      core.restore_terminal_focus()
    end
    return
  end
  leave_terminal()
  local function go(sock)
    if not sock or sock == "" then
      core.restore_terminal_focus()
      return
    end
    local function finish()
      session.record_last(entry.path)
      vim.cmd("connect " .. vim.fn.fnameescape(sock))
    end
    if view then
      local expr = ("luaeval('(function() require([[mux]]).open_view([[%s]]) return 1 end)()')"):format(view)
      vim.system({ "nvim", "--server", sock, "--remote-expr", expr }, function() vim.schedule(finish) end)
    else
      finish()
    end
  end
  if entry.socket and entry.socket ~= "" then
    go(entry.socket)
    return
  end
  vim.system({ "mux", "ensure", entry.path }, { text = true }, function(res)
    local sock = res.code == 0 and res.stdout and res.stdout:match "[^\n]+" or nil
    vim.schedule(function()
      if sock then
        go(sock)
      else
        core.restore_terminal_focus()
      end
    end)
  end)
end

function M.pick_project()
  leave_terminal()
  local ok = pcall(vim.system, { "mux", "list" }, { text = true }, function(res)
    vim.schedule(function()
      if res.code == 0 then
        show_picker(parse_list_output(res.stdout))
      else
        core.restore_terminal_focus()
      end
    end)
  end)
  if not ok then core.restore_terminal_focus() end
end

function M.pick_project_all()
  leave_terminal()
  local ok = pcall(vim.system, { "mux", "list", "--all" }, { text = true }, function(res)
    vim.schedule(function()
      if res.code == 0 then
        show_picker(parse_list_all_output(res.stdout), { federated = true })
      else
        core.restore_terminal_focus()
      end
    end)
  end)
  if not ok then core.restore_terminal_focus() end
end

---@param step integer 1 = next live project, -1 = previous (wraps)
function M.cycle_project(step)
  local entries = {}
  for _, item in ipairs(list_entries()) do
    if item.status == "live" and item.socket ~= "" then
      entries[#entries + 1] = { cwd = item.cwd, sock = item.socket }
    end
  end
  vim.schedule(function()
    if #entries < 2 then return end
    local cur, idx = vim.v.servername, 1
    for i, e in ipairs(entries) do
      if e.sock == cur then
        idx = i
        break
      end
    end
    local target = entries[((idx - 1 + step) % #entries) + 1]
    if not target or target.sock == cur then return end
    leave_terminal()
    session.record_last(target.cwd)
    vim.cmd("connect " .. vim.fn.fnameescape(target.sock))
  end)
end

---@param cb fun(root?: string, sock?: string)
local function with_latest_live_other(cb)
  local live = {}
  for _, item in ipairs(list_entries()) do
    if item.cwd and item.status == "live" and item.socket ~= "" then live[canon(item.cwd)] = item.socket end
  end
  vim.schedule(function()
    local cur = session.root()
    local hf = core.state_dir() .. "/history"
    local hist = vim.fn.filereadable(hf) == 1 and vim.fn.readfile(hf) or {}
    for i = #hist, 1, -1 do
      local root = canon(hist[i])
      local sock = root ~= "" and root ~= cur and live[root]
      if sock then
        cb(root, sock)
        return
      end
    end
    cb(nil, nil)
  end)
end

function M.last_session()
  with_latest_live_other(function(root, sock)
    if sock then M._connect { path = root, socket = sock } end
  end)
end

-- Killing the last tabpage: hop the client to the latest live project, then
-- soft-stop this session so it stays resumable. With no other live project the
-- stop drops the client to the shell -- like tmux detach-on-destroy=off.
function M.stop_to_latest()
  with_latest_live_other(function(root, sock)
    if sock then
      session.record_last(root)
      pcall(vim.cmd, "connect " .. vim.fn.fnameescape(sock))
    end
    session.stop_session()
  end)
end

function M.kill_to_latest()
  local current = session.root()
  confirm_kill(current, function()
    with_latest_live_other(function(root, sock)
      if sock then
        session.record_last(root)
        pcall(vim.cmd, "connect " .. vim.fn.fnameescape(sock))
      end
      session.kill_session()
    end)
  end)
end

return M
