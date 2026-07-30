-- pr.list: the PR home surface - one buffer (pr://list), one line per PR.
--
-- Stacks are detected from branch topology alone: a PR whose base branch is
-- another open PR's head branch is that PR's child, and renders indented
-- under it. The flat display order is exported as M.order, and ]p / [p in
-- the files view walk exactly that order - so reviewing a stack is just
-- stepping through adjacent lines.
--
-- Canola-style: the buffer IS the list. <CR> dives into a PR (pr://files),
-- "-" in the files view comes back up here.

local data = require "pr.data"
local fmt = require "pr.fmt"

local M = {}

local buf ---@type integer?
local ns = vim.api.nvim_create_namespace "pr_list"

M.order = {} ---@type table[] PRs in display order (stacks adjacent, parent first)
M.root = nil ---@type string?

local function warn(msg) vim.notify("pr: " .. msg, vim.log.levels.WARN) end

-- -------------------------------------------------------------------- orb ---

--- CI state as a colored orb. Plain UTF-8 - no icon font required (nonicons
--- glyphs are PUA codepoints that collide with Nerd Fonts: hello bluetooth).
local ORB = {
  success = { "●", "DiagnosticOk" },
  failure = { "●", "DiagnosticError" },
  pending = { "●", "DiagnosticWarn" },
}
local function orb(p)
  local o = p.ci and ORB[p.ci]
  if o then return o[1], o[2] end
  return "○", "Comment" -- no CI on this PR
end

-- ------------------------------------------------------------------ stack ---

--- Order PRs so stack members are adjacent, parent first, children indented.
--- Detection: base branch == another open PR's head branch => child.
---@param prs table[]
---@return table[] order each entry gains a `depth` field (0 = root)
local function stacked(prs)
  local by_head, children, roots = {}, {}, {}
  for _, p in ipairs(prs) do
    if p.headRefName and p.headRefName ~= "" then by_head[p.headRefName] = p end
  end
  for _, p in ipairs(prs) do
    local parent = p.baseRefName and by_head[p.baseRefName]
    if parent and parent.number ~= p.number then
      children[parent.number] = children[parent.number] or {}
      table.insert(children[parent.number], p)
    else
      roots[#roots + 1] = p
    end
  end
  local order, seen = {}, {}
  local function walk(p, depth)
    if seen[p.number] then return end
    seen[p.number] = true
    p.depth = depth
    order[#order + 1] = p
    for _, c in ipairs(children[p.number] or {}) do
      walk(c, depth + 1)
    end
  end
  for _, p in ipairs(roots) do
    walk(p, 0)
  end
  -- Base cycles (a based on b, b based on a) leave no root: append flat.
  for _, p in ipairs(prs) do
    walk(p, 0)
  end
  return order
end

-- ----------------------------------------------------------------- render ---

--- Last rendered PR set, so resize re-renders re-flow the columns.
local last ---@type table[]?

local function render(prs)
  last = prs
  M.order = stacked(prs)

  -- fzf-picker-era columns: number | title (fills the window) | author | age,
  -- author + age right-aligned against the window edge. Title absorbs
  -- whatever the fixed columns leave, truncating into "…" when narrow.
  local win = vim.fn.bufwinid(buf)
  local width = win ~= -1 and (vim.api.nvim_win_get_width(win) - vim.fn.getwininfo(win)[1].textoff) or vim.o.columns
  local num_w, author_w, age_w, gap = 6, 16, 4, 2
  -- 3 = leading space + orb + space; all widths in display cells.
  local title_w = math.max(20, width - 3 - num_w - author_w - age_w - 3 * gap)

  local lines, marks = {}, {}
  --- Build a line from { text, group? } segments; offsets computed, never
  --- hand-counted (same shape as pr.view).
  ---@param segs {[1]:string,[2]:string?}[]
  local function seg(segs)
    local text, off = {}, 0
    for _, sg in ipairs(segs) do
      text[#text + 1] = sg[1]
      if sg[2] then marks[#marks + 1] = { #lines, off, off + #sg[1], sg[2] } end
      off = off + #sg[1]
    end
    lines[#lines + 1] = table.concat(text)
  end

  for _, p in ipairs(M.order) do
    local glyph, hl = orb(p)
    local connector = p.depth > 0 and (string.rep("  ", p.depth - 1) .. "└ ") or ""
    local title = fmt.trunc(connector .. (p.title or ""), title_w)
    seg {
      { " " },
      { glyph, hl },
      { " " },
      { ("#%-5d"):format(p.number), p.isDraft and "Comment" or "DiagnosticOk" }, -- grey = draft
      { "  " },
      { connector, "Constant" }, -- └ in purple (CozyboxPurple)
      { title:sub(#connector + 1) },
      { string.rep(" ", math.max(0, title_w - vim.fn.strdisplaywidth(title))) },
      { "  " },
      { fmt.rjust(p.author and p.author.login or "?", 16), "Identifier" },
      { "  " },
      { fmt.rjust(fmt.ago(p.updatedAt), 4), "Comment" },
    }
  end
  if #lines == 0 then lines = { " no open PRs" } end

  vim.bo[buf].modifiable = true
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  vim.bo[buf].modified = false -- a render is a load, never an edit (:e checks this)
  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  for _, m in ipairs(marks) do
    vim.api.nvim_buf_set_extmark(buf, ns, m[1], m[2], { end_col = m[3], hl_group = m[4] })
  end
end

--- Re-paint the last fetched set with no network call. The verbs mutate PR
--- tables in place (draft flips a highlight, nothing else), so this is all
--- the redraw they need.
function M.repaint()
  if last and buf and vim.api.nvim_buf_is_valid(buf) then render(last) end
end

--- Force the next M.open to re-fetch, without touching the buffer now.
function M.invalidate() M.order = {} end

--- Stack membership for a PR number, read straight off the display order
--- (a stack = a depth-0 root plus the depth>0 run under it). Feeds the
--- files-view "Stack: 2/3" counter as ]p / [p walk the list.
---@param number integer
---@return {pos:integer, total:integer, parent:table?}? nil when not in a stack
function M.stack_info(number)
  local i0
  for i, p in ipairs(M.order) do
    if p.number == number then
      i0 = i
      break
    end
  end
  if not i0 then return end
  local root = i0
  while M.order[root].depth > 0 do
    root = root - 1
  end
  local last = root
  while M.order[last + 1] and M.order[last + 1].depth > 0 do
    last = last + 1
  end
  if last == root then return end -- not stacked
  local me, parent = M.order[i0], nil
  for i = i0 - 1, root, -1 do
    if M.order[i].depth == me.depth - 1 then
      parent = M.order[i]
      break
    end
  end
  return { pos = i0 - root + 1, total = last - root + 1, parent = me.depth > 0 and parent or nil }
end

--- Land the cursor on the currently loaded PR, if it is in the list.
local function focus_current()
  local cur = require("pr").state.pr
  if not cur then return end
  for i, p in ipairs(M.order) do
    if p.number == cur.number then return pcall(vim.api.nvim_win_set_cursor, 0, { i, 0 }) end
  end
end

-- ------------------------------------------------------------------- help ---

local HELP = {
  { "g?", "this help" },
  { "<CR>", "review PR (pr://files)" },
  { "]p / [p", "next / prev PR (global)" },
  { "R / :e", "refresh" },
  { "q", "back" },
}

--- Verb rows are appended from pr.verbs so the two surfaces can never drift.
local function help()
  local rows = vim.deepcopy(HELP)
  vim.list_extend(rows, require("pr.verbs").help_entries())
  fmt.help(" pr list ", rows)
end

-- ------------------------------------------------------------------- open ---

local function select()
  local p = M.order[vim.fn.line "."]
  if p and M.root then require("pr").load(M.root, p) end
end

--- `:e pr://list` in a fresh session lands nvim on a brand new buffer
--- already wearing that name; creating our own on top would be a duplicate
--- name (E95). Adopt whatever buffer is already there instead.
local function ensure_buf()
  if buf and vim.api.nvim_buf_is_valid(buf) then return end
  local cur = vim.api.nvim_get_current_buf()
  local adopt = vim.api.nvim_buf_get_name(cur):match "pr://list$" ~= nil
  buf = adopt and cur or vim.api.nvim_create_buf(false, true)
  if not adopt then vim.api.nvim_buf_set_name(buf, "pr://list") end
  vim.bo[buf].buftype = "nofile"
  vim.bo[buf].bufhidden = "hide"
  vim.bo[buf].swapfile = false
  vim.bo[buf].filetype = "prlist"
  vim.bo[buf].modifiable = false

  -- Columns derive from the window width: re-flow when that changes, or when
  -- the buffer is re-shown in a differently sized window.
  local grp = vim.api.nvim_create_augroup("pr_list_layout", { clear = true })
  vim.api.nvim_create_autocmd({ "WinResized", "VimResized" }, {
    group = grp,
    callback = function()
      if last and buf and vim.api.nvim_buf_is_valid(buf) and vim.fn.bufwinid(buf) ~= -1 then render(last) end
    end,
  })
  vim.api.nvim_create_autocmd("BufWinEnter", {
    group = grp,
    buffer = buf,
    callback = function()
      if last then render(last) end
    end,
  })

  local o = { buffer = buf, silent = true }
  vim.keymap.set("n", "<CR>", select, o)
  vim.keymap.set("n", "R", function() M.open(true) end, o)
  vim.keymap.set("n", "g?", help, o)
  vim.keymap.set("n", "q", "<cmd>silent! buffer #<cr>", o)
  require("pr.verbs").attach(buf)
end

--- Drops stale async responses after an R mid-flight.
local generation = 0

--- Open the list; the buffer caches the last fetch (guh-style: load eagerly,
--- refetch only on explicit refresh). First open and R fetch.
---
--- Two-phase load: the fast metadata list renders immediately, then the CI
--- states (the slow half - see data.ci) trail in and recolor the orbs.
---@param refresh? boolean
function M.open(refresh)
  local root = data.root()
  if not root then return warn "not in a git repository" end
  if not data.has_refspec(root) then
    if not data.install_refspec(root) then return warn "could not write git config" end
  end

  ensure_buf()
  if vim.api.nvim_get_current_buf() ~= buf then vim.api.nvim_win_set_buf(0, buf) end

  if #M.order > 0 and M.root == root and not refresh then return focus_current() end

  M.root = root
  generation = generation + 1
  local gen = generation
  vim.bo[buf].modifiable = true
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, { " loading PRs..." })
  vim.bo[buf].modifiable = false

  data.prs(root, function(prs, err)
    if gen ~= generation or not (buf and vim.api.nvim_buf_is_valid(buf)) then return end
    if not prs then return warn(err or "could not list PRs") end
    render(prs)
    if vim.api.nvim_get_current_buf() == buf then focus_current() end

    data.ci(root, function(map)
      if gen ~= generation or not map or not (buf and vim.api.nvim_buf_is_valid(buf)) then return end
      local changed = false
      for _, p in ipairs(M.order) do
        if map[p.number] ~= p.ci then
          p.ci, changed = map[p.number], true
        end
      end
      -- Re-render in place: line count is unchanged, so the cursor stays put.
      if changed then render(prs) end
    end)
  end)
end

return M
