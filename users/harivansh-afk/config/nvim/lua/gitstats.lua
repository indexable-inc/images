-- gitstats: THE definition of the right-aligned "+N -N" LOC virtual text
-- shared by pr://files rows and fugitive status rows. One color source and
-- one segment shape here so the two views can never drift apart.
--
-- Stat colors are TAKEN FROM diffs.nvim (its rail line-number fg is its
-- green/red identity), falling back to Added/Removed without it. The delete
-- side is darkened a notch: the raw rail red reads too hot next to the
-- green when it is just a number at right-align distance.

local M = {}

local ns = vim.api.nvim_create_namespace "gitstats"

-- ------------------------------------------------------------ highlights ---

--- 0xRRGGBB scaled toward black.
local function darken(fg, factor)
  local r = math.floor(math.floor(fg / 0x10000) % 0x100 * factor)
  local g = math.floor(math.floor(fg / 0x100) % 0x100 * factor)
  local b = math.floor(fg % 0x100 * factor)
  return r * 0x10000 + g * 0x100 + b
end

local DEL_DARKEN = 0.8

--- First defined fg among `names`, nil when none resolves.
local function fg_of(names)
  for _, name in ipairs(names) do
    local ok, h = pcall(vim.api.nvim_get_hl, 0, { name = name, link = false })
    if ok and h and h.fg then return h.fg end
  end
end

--- (Re-)derive GitStatAdd/GitStatDel. Cheap; callers run it per render so a
--- colorscheme switch or a late diffs.nvim load is picked up next paint.
function M.setup_hls()
  local add = fg_of { "DiffsAddRailNr", "Added" }
  local del = fg_of { "DiffsDeleteRailNr", "Removed" }
  vim.api.nvim_set_hl(0, "GitStatAdd", add and { fg = add } or { link = "Added" })
  vim.api.nvim_set_hl(0, "GitStatDel", del and { fg = darken(del, DEL_DARKEN) } or { link = "Removed" })
end

--- virt_text segments for one file row - the ONE definition of the shape.
---@param add integer
---@param del integer
---@param binary? boolean
---@return {[1]: string, [2]: string}[]
function M.virt(add, del, binary)
  if binary then return { { "binary", "Comment" } } end
  return { { "+" .. add, "GitStatAdd" }, { " ", "Normal" }, { "-" .. del, "GitStatDel" } }
end

-- ---------------------------------------------------- fugitive decoration ---
-- Fugitive's status buffer is decorated, never edited: stats live in
-- extmark virtual text in our own namespace, so fugitive's renders, maps
-- and inline `=` diffs are untouched. Hooked on User FugitiveIndex, which
-- fugitive#BufReadStatus fires at the end of EVERY status render (first
-- load and every reload) once the buffer lines are final.

--- Section heading -> which diff answers for its rows.
local SECTIONS = { Untracked = "untracked", Unstaged = "worktree", Staged = "index" }

-- Untracked stats cost one `git diff --no-index` process per file; beyond
-- this many, the remaining untracked rows simply stay bare.
local UNTRACKED_MAX = 64

--- Parse `git diff --numstat -z` records into path -> stats.
--- Record shapes (verified against git 2.x):
---   add\tdel\tpath\0            plain change
---   add\tdel\t\0old\0new\0      rename/copy (and --no-index pairs)
--- Keyed by the LAST path of the record - the post-image name, which is
--- what a fugitive row displays. Binary files carry "-" counts.
---@return table<string, {add: integer, del: integer, binary: boolean}>
local function parse_numstat(out)
  local stats, i = {}, 1
  while i <= #out do
    local nul = out:find("\0", i, true)
    if not nul then break end
    local add, del, path = out:sub(i, nul - 1):match "^([%d-]+)\t([%d-]+)\t(.*)$"
    if not add then break end
    i = nul + 1
    if path == "" then -- two NUL-terminated paths follow: old, then new
      local mid = out:find("\0", i, true)
      local fin = mid and out:find("\0", mid + 1, true)
      if not fin then break end
      path, i = out:sub(mid + 1, fin - 1), fin + 1
    end
    stats[path] = { add = tonumber(add) or 0, del = tonumber(del) or 0, binary = add == "-" }
  end
  return stats
end

--- File rows of a fugitive status buffer: section tracking follows the
--- `Heading (N)` lines, and only rows inside Untracked/Unstaged/Staged are
--- files - so rebase/log lines and inline `=` diff lines can never match.
--- Rename rows display "old -> new"; the stat key is the new path.
---@return {row: integer, path: string, section: string}[]
local function file_rows(buf)
  local rows, section = {}, nil
  for i, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
    local heading = line:match "^(.+) %(%d+%)$"
    if heading then
      section = SECTIONS[heading] -- unknown headings (Unpushed/Rebasing/...) end the section
    elseif section then
      local name = line:match "^[MADRCU?!] (.+)$"
      if name then rows[#rows + 1] = { row = i, path = name:match "^.* %-> (.+)$" or name, section = section } end
    end
  end
  return rows
end

--- Decorate a fugitive status buffer with per-file "+N -N" stats.
--- Sources match fugitive's own section semantics exactly:
---   Unstaged  -> git diff --numstat            (worktree vs index)
---   Staged    -> git diff --numstat --cached   (index vs HEAD, renames on)
---   Untracked -> git diff --numstat --no-index /dev/null <file>
--- All diffs run async. Rows are re-parsed AT PLACEMENT time (rows may have
--- shifted under inline `=` diffs or a re-render meanwhile; a path that no
--- longer has a row simply places nothing), and a per-buffer generation
--- counter lets only the newest decoration write - every status render
--- fires FugitiveIndex again, so stale stats never outlive one render.
---@param buf integer
function M.fugitive(buf)
  if not vim.api.nvim_buf_is_valid(buf) then return end
  M.setup_hls()
  local tree = vim.fn.FugitiveWorkTree(buf)
  if tree == "" then return end

  local gen = (vim.b[buf].gitstats_gen or 0) + 1
  vim.b[buf].gitstats_gen = gen
  local stats = { untracked = {} } ---@type table<string, table<string, table>>

  local wanted = { worktree = false, index = false } -- only diff sections that have rows
  local untracked = {}
  for _, r in ipairs(file_rows(buf)) do
    if r.section == "untracked" then
      if not r.path:find "/$" and #untracked < UNTRACKED_MAX then untracked[#untracked + 1] = r.path end
    else
      wanted[r.section] = true
    end
  end

  local pending = (wanted.worktree and 1 or 0) + (wanted.index and 1 or 0) + #untracked
  local function place()
    if not vim.api.nvim_buf_is_valid(buf) or vim.b[buf].gitstats_gen ~= gen then return end
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
    for _, r in ipairs(file_rows(buf)) do
      local s = stats[r.section] and stats[r.section][r.path]
      if s then
        vim.api.nvim_buf_set_extmark(buf, ns, r.row - 1, 0, {
          virt_text = M.virt(s.add, s.del, s.binary),
          virt_text_pos = "right_align",
        })
      end
    end
  end
  if pending == 0 then return place() end -- nothing to diff: just clear stale marks

  local function run(args, on_out)
    vim.system(
      vim.list_extend({ "git", "--no-optional-locks", "-C", tree }, args),
      { text = true },
      vim.schedule_wrap(function(r) -- no-index exits 1 on any diff; only the bytes matter
        on_out(r.stdout or "")
        pending = pending - 1
        if pending == 0 then place() end
      end)
    )
  end

  if wanted.worktree then run({ "diff", "--numstat", "-z" }, function(out) stats.worktree = parse_numstat(out) end) end
  if wanted.index then
    run({ "diff", "--numstat", "-z", "-M", "--cached" }, function(out) stats.index = parse_numstat(out) end)
  end
  for _, path in ipairs(untracked) do
    run({ "diff", "--numstat", "-z", "--no-index", "--", "/dev/null", path }, function(out)
      -- Record is keyed by the post-image path (== `path`); an empty diff
      -- (empty file) still deserves an explicit +0 -0.
      stats.untracked[path] = parse_numstat(out)[path] or { add = 0, del = 0, binary = false }
    end)
  end
end

return M
