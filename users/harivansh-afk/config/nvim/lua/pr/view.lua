-- pr.view: THE surface for PR review - a fugitive-status style files buffer.
--
-- One row per file ("M path", letter semantics identical to fugitive),
-- <Tab>/= expands the file's hunks inline, <CR> opens the full diffs.nvim
-- review at that file. The buffer is a pure function of require("pr").state
-- plus the `expanded` set: every change rebuilds it wholesale.
--
-- diffs.nvim decoration: rows are exactly "M path" because its parser's
-- FIRST filename pattern is the fugitive form `^[MADRCU?!]%s+(.+)$` - clean
-- rows give every expanded hunk correct treesitter language highlighting.
-- Stats live in right-aligned virtual text so they never pollute the row.
--
-- Colors are fugitive's, exactly: every group here routes through the
-- fugitive* highlight groups (declared below with fugitive's own default
-- links), so the buffer is indistinguishable from a :Git status palette
-- and follows any colorscheme that themes fugitive.

local data = require "pr.data"
local gitstats = require "gitstats"

local M = {}

local ns = vim.api.nvim_create_namespace "pr_view"
local buf = nil ---@type integer?
local attached = false
local expanded = {} ---@type table<string, true> -- survives commit hops on purpose
local line_map = {} ---@type table<integer, string> -- lnum -> owning file path
local file_rows = {} ---@type table<string, integer> -- path -> its row lnum

local function state() return require("pr").state end

function M.active() return buf ~= nil and vim.api.nvim_get_current_buf() == buf end

-- ------------------------------------------------------------ highlights ---

--- fugitive's exact hi-def-link table (syntax/fugitive.vim), declared with
--- default=true: the view renders IDENTICALLY to a :Git status buffer even
--- before fugitive's syntax ever loads, real fugitive links are no-ops on
--- top of these, and a colorscheme that themes fugitive restyles this view
--- for free.
local FUGITIVE_LINKS = {
  fugitiveHeader = "Label",
  fugitiveHelpHeader = "fugitiveHeader",
  fugitiveHelpTag = "Tag",
  fugitiveHeading = "PreProc",
  fugitiveUntrackedHeading = "PreCondit",
  fugitiveUnstagedHeading = "Macro",
  fugitiveStagedHeading = "Include",
  fugitiveModifier = "Type",
  fugitiveUntrackedModifier = "StorageClass",
  fugitiveUnstagedModifier = "Structure",
  fugitiveStagedModifier = "Typedef",
  fugitiveHash = "Identifier",
  fugitiveSymbolicRef = "Function",
  fugitiveCount = "Number",
}

--- Letter -> the fugitive modifier group that letter wears in a fugitive
--- status buffer. fugitive colors by SECTION, not letter; a PR file list has
--- one section, so each letter takes the group of the section it lives in
--- in practice: M/R/C sit in Unstaged (Structure), A lands in Staged
--- (Typedef), D/? borrow Untracked (StorageClass) to stay distinct without
--- leaving fugitive's palette. Anything else falls back to fugitiveModifier.
local MOD_HL = {
  M = "fugitiveUnstagedModifier",
  R = "fugitiveUnstagedModifier",
  C = "fugitiveUnstagedModifier",
  A = "fugitiveStagedModifier",
  D = "fugitiveUntrackedModifier",
  ["?"] = "fugitiveUntrackedModifier",
}

--- Stat colors (GitStatAdd/GitStatDel) come from gitstats - the ONE module
--- defining the "+N -N" identity shared with the fugitive status buffer.
local function setup_hls()
  vim.api.nvim_set_hl(0, "diffLine", { link = "Statement", default = true })

  for group, link in pairs(FUGITIVE_LINKS) do
    vim.api.nvim_set_hl(0, group, { link = link, default = true })
  end

  gitstats.setup_hls()
end

-- ---------------------------------------------------------------- render ---

---@param keep? string  path to keep the cursor on
function M.render(keep)
  local s = state()
  local c = s.commits[s.idx]
  if not (buf and c) then return end

  setup_hls()
  -- Resolve the base ref ONCE per render: diff caches key on shas, so an
  -- externally moved origin/<base> invalidates them without any bookkeeping.
  local base_sha = s.mode == "cumulative" and data.resolve(s.root, s.base) or nil
  local files = data.files(s.root, s.base, c.sha, s.mode, base_sha)
  local has_diffs = (pcall(require, "diffs"))

  -- Fugitive-style eagerness: its status render starts the section diffs it
  -- will need for inline expansion BEFORE anyone asks. Same here - this
  -- range's whole diff starts now, in the background, so the first <Tab>
  -- reads cache; the neighbor commits' file tables warm for ]c / [c.
  data.warm_diff(s.root, s.base, c.sha, s.mode, base_sha)
  for _, i in ipairs { s.idx - 1, s.idx + 1 } do
    if s.commits[i] then data.warm_files(s.root, s.base, s.commits[i].sha, s.mode, base_sha) end
  end

  local lines, hl, vt = {}, {}, {}
  line_map, file_rows = {}, {}

  --- Build a line from { text, group? } segments; offsets computed, never
  --- hand-counted.
  ---@param segs {[1]:string,[2]:string?}[]
  local function seg(segs)
    local text, off = {}, 0
    for _, sg in ipairs(segs) do
      text[#text + 1] = sg[1]
      if sg[2] then hl[#hl + 1] = { #lines, sg[2], off, off + #sg[1] } end
      off = off + #sg[1]
    end
    lines[#lines + 1] = table.concat(text)
  end

  -- Identifier = blue in cozybox, same as the author column in pr://list.
  seg { { "Author:", "fugitiveHeader" }, { " " }, { s.pr.author and s.pr.author.login or "?", "Identifier" } }
  -- Draft is carried by a GREY number, exactly as in pr://list and the fzf
  -- picker - no "[draft]" column anywhere in this plugin.
  seg {
    { "PR:", "fugitiveHeader" },
    { "     " },
    { "#" .. s.pr.number, s.pr.isDraft and "Comment" or "fugitiveCount" },
    { " " },
    { s.pr.title or "" },
  }
  seg {
    { "Commit:", "fugitiveHeader" },
    { " " },
    { c.sha, "fugitiveHash" },
    { " " },
    { c.subject },
  }
  seg {
    { "Base:", "fugitiveHeader" },
    { "   " },
    { s.base or "", "fugitiveSymbolicRef" },
  }
  seg {
    { "Mode:", "fugitiveHeader" },
    { "   " },
    { s.mode, "fugitiveSymbolicRef" },
    { " (" },
    { ("%d/%d"):format(s.idx, #s.commits), "fugitiveCount" },
    { ")" },
  }
  -- Stack position, when pr://list knows this PR is stacked. ]p / [p move
  -- this counter the way ]c / [c move the Mode one.
  local stack = package.loaded["pr.list"] and require("pr.list").stack_info(s.pr.number)
  if stack then
    seg {
      { "Stack:", "fugitiveHeader" },
      { "  " },
      { ("%d/%d"):format(stack.pos, stack.total), "fugitiveCount" },
      stack.parent and { (" └ #%d"):format(stack.parent.number), "Constant" } or { "" },
    }
  end
  seg { { "Help:", "fugitiveHelpHeader" }, { "   " }, { "g?", "fugitiveHelpTag" } }
  lines[#lines + 1] = ""
  -- Fugitive-bare heading: change totals live ONLY in the per-file virtual
  -- text, never duplicated up here.
  seg {
    { "Files", "fugitiveHeading" },
    { " (" },
    { tostring(#files), "fugitiveCount" },
    { ")" },
  }

  for _, f in ipairs(files) do
    local name = f.old_path and (f.old_path .. " -> " .. f.path) or f.path
    seg { { f.status, MOD_HL[f.status] or "fugitiveModifier" }, { " " }, { name } } -- modifier + plain path
    local row = #lines
    line_map[row], file_rows[f.path] = f.path, row
    vt[#vt + 1] = { row - 1, gitstats.virt(f.add, f.del, f.binary) }

    if expanded[f.path] then
      for _, l in ipairs(data.file_diff(s.root, s.base, c.sha, s.mode, f.path, base_sha)) do
        local at = l:match "^@@+.-@@+"
        -- Drop git's xfuncname context from hunk headers: the marker line is
        -- a delimiter, code belongs to the hunk body below it.
        if at then l = at end
        lines[#lines + 1] = l
        line_map[#lines] = f.path
        if at then
          hl[#hl + 1] = { #lines - 1, "diffLine", 0, #at }
        elseif not has_diffs then
          -- Flat fallback ONLY without diffs.nvim - never fight its treesitter.
          local ch = l:sub(1, 1)
          local g = ch == "+" and "GitStatAdd" or ch == "-" and "GitStatDel" or nil
          if g then hl[#hl + 1] = { #lines - 1, g, 0, -1 } end
        end
      end
      lines[#lines + 1] = ""
    end
  end

  vim.bo[buf].modifiable = true
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  vim.bo[buf].modified = false -- a render is a load, never an edit (:e checks this)

  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  for _, h in ipairs(hl) do
    local end_col = h[4] == -1 and #lines[h[1] + 1] or h[4]
    vim.api.nvim_buf_set_extmark(buf, ns, h[1], h[3], { end_col = end_col, hl_group = h[2] })
  end
  for _, v in ipairs(vt) do
    vim.api.nvim_buf_set_extmark(buf, ns, v[1], 0, { virt_text = v[2], virt_text_pos = "right_align" })
  end

  local diffs_ok, diffs = pcall(require, "diffs")
  if diffs_ok then
    -- diffs.nvim's parser reads b:diffs_repo_root to resolve filetypes (open
    -- buffers, then filename, then file CONTENT under the root) - without it
    -- the treesitter + intra-line diffing falls back to cwd guessing.
    vim.b[buf].diffs_repo_root = s.root
    if attached then
      diffs.refresh(buf)
    else
      diffs.attach(buf)
      attached = true
    end
  end

  local target = keep and file_rows[keep]
  if target then vim.api.nvim_win_set_cursor(0, { target, 0 }) end
end

-- --------------------------------------------------------------- actions ---

local function toggle()
  local path = line_map[vim.fn.line "."]
  if not path then return end
  expanded[path] = not expanded[path] and true or nil
  M.render(path)
end

--- New-file line for a cursor row inside an expanded hunk, nil elsewhere.
--- Walk up to the owning @@ header, then count new-side rows (context and
--- '+'; '-' rows do not exist in the new file) down to the cursor.
local function hunk_line(row, path)
  local frow = file_rows[path]
  if not (frow and buf) or row <= frow then return nil end
  local lines = vim.api.nvim_buf_get_lines(buf, frow, row, false) -- rows frow+1..row
  for i = #lines, 1, -1 do
    local start = lines[i]:match "^@@+ %-%d+,?%d* %+(%d+)"
    if start then
      if i == #lines then return tonumber(start) end -- cursor ON the header
      local n = 0
      for j = i + 1, #lines do
        if lines[j]:sub(1, 1) ~= "-" then n = n + 1 end
      end
      -- A '-' row has no new-side self: land on the first line after it.
      return tonumber(start) + n - (lines[#lines]:sub(1, 1) ~= "-" and 1 or 0)
    end
  end
  return nil
end

--- Full diffs.nvim review of the current range, jumped to this file - and,
--- from inside an expanded hunk, to this line. review_goto only takes a
--- file key (no line targeting in diffs.nvim), but the review split shows
--- real file content, so landing is a plain cursor move after the switch.
local function dive()
  local row = vim.fn.line "."
  local path = line_map[row]
  if not path then return end
  local lnum = hunk_line(row, path)
  local s = state()
  local c = s.commits[s.idx]
  local ok, commands = pcall(require, "diffs.commands")
  if not (ok and c) then return end
  commands.review(data.spec(s.root, s.base, c.sha, s.mode))
  vim.schedule(function()
    pcall(require("diffs").review_goto, path)
    if lnum then
      vim.schedule(function()
        pcall(vim.api.nvim_win_set_cursor, 0, { lnum, 0 })
        vim.cmd "silent! normal! zvzz"
      end)
    end
  end)
end

--- g? - the fugitive gesture. A small float over the cursor, keys in
--- fugitiveHelpTag like fugitive's own help column.
local HELP = {
  { "g?", "this help" },
  { "<Tab> / =", "toggle inline hunks" },
  { "<CR>", "open full review at file / line" },
  { "]f / [f", "next / prev file" },
  { "]c / [c", "next / prev commit" },
  { "]p / [p", "next / prev PR" },
  { "-", "PR list" },
  { "<leader>m", "cumulative <-> incremental" },
  { "<leader>gC", "pick commit" },
  { "<leader>gA", "whole PR view" },
  { "R / :e", "re-fetch this PR" },
  { "q", "back" },
}

--- Verb rows are appended from pr.verbs so the two surfaces can never drift.
local function help()
  local rows = vim.deepcopy(HELP)
  vim.list_extend(rows, require("pr.verbs").help_entries())
  require("pr.fmt").help(" pr ", rows)
end

---@param dir 1|-1
local function file_step(dir)
  local cur = vim.fn.line "."
  local best
  for _, row in pairs(file_rows) do
    if dir == 1 and row > cur and (not best or row < best) then best = row end
    if dir == -1 and row < cur and (not best or row > best) then best = row end
  end
  if best then vim.api.nvim_win_set_cursor(0, { best, 0 }) end
end

-- ------------------------------------------------------------------ open ---

--- With 'number' on, the gutter width follows line('$'): expanding a big
--- file's hunks pushes the buffer past 999 lines and the number column
--- widens, shifting every row sideways on <Tab>. Relative numbers never
--- exceed the window height, so 'number' off + whatever 'relativenumber'
--- the user runs = a CONSTANT gutter. Window options outlive the buffer in
--- that window, so save on enter and restore on leave.
local WIN_OPTS = { wrap = false, number = false }
local saved_win ---@type table<string, any>?

local function win_opts_apply()
  if saved_win then return end -- already applied in this window
  saved_win = {}
  for opt, val in pairs(WIN_OPTS) do
    saved_win[opt] = vim.api.nvim_get_option_value(opt, { win = 0 })
    vim.api.nvim_set_option_value(opt, val, { win = 0, scope = "local" })
  end
end

local function win_opts_restore()
  for opt, val in pairs(saved_win or {}) do
    vim.api.nvim_set_option_value(opt, val, { win = 0, scope = "local" })
  end
  saved_win = nil
end

--- `:e pr://files` in a fresh session lands nvim on a brand new buffer
--- already wearing that name; creating our own on top would be a duplicate
--- name (E95). Adopt whatever buffer is already there instead.
local function ensure_buf()
  if buf and vim.api.nvim_buf_is_valid(buf) then return end
  local cur = vim.api.nvim_get_current_buf()
  local adopt = vim.api.nvim_buf_get_name(cur):match "pr://files$" ~= nil
  buf = adopt and cur or vim.api.nvim_create_buf(false, true)
  attached = false
  if not adopt then vim.api.nvim_buf_set_name(buf, "pr://files") end
  vim.bo[buf].buftype = "nofile"
  vim.bo[buf].bufhidden = "hide"
  vim.bo[buf].swapfile = false
  vim.bo[buf].filetype = "prfiles"

  vim.api.nvim_create_autocmd("BufEnter", { buffer = buf, callback = win_opts_apply })
  vim.api.nvim_create_autocmd("BufLeave", { buffer = buf, callback = win_opts_restore })

  local o = { buffer = buf, silent = true }
  vim.keymap.set("n", "<Tab>", toggle, o)
  vim.keymap.set("n", "=", toggle, o)
  vim.keymap.set("n", "<CR>", dive, o)
  vim.keymap.set("n", "]f", function() file_step(1) end, o)
  vim.keymap.set("n", "[f", function() file_step(-1) end, o)
  vim.keymap.set("n", "R", function() require("pr").reload() end, o)
  vim.keymap.set("n", "-", function() require("pr.list").open() end, o)
  vim.keymap.set("n", "g?", help, o)
  vim.keymap.set("n", "q", "<cmd>silent! buffer #<cr>", o)
  require("pr.verbs").attach(buf)
end

function M.open()
  ensure_buf()
  local was_active = M.active()
  if not was_active then vim.api.nvim_win_set_buf(0, buf) end -- fires BufEnter -> win_opts_apply
  M.render()
  if not was_active then
    -- Land on the first file row, wherever the header ends.
    local first = math.huge
    for _, row in pairs(file_rows) do
      first = math.min(first, row)
    end
    if first == math.huge then first = vim.api.nvim_buf_line_count(buf) end
    vim.api.nvim_win_set_cursor(0, { math.min(first, vim.api.nvim_buf_line_count(buf)), 0 })
  end
end

return M
