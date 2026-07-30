-- pr.pick: the two fzf surfaces (PR list, commit list).
--
-- Pure presentation: everything here formats entries and hands the selection
-- back to require("pr") - no git state lives in this module.

local data = require "pr.data"

local M = {}

local function warn(msg) vim.notify("pr: " .. msg, vim.log.levels.WARN) end
local function info(msg) vim.notify("pr: " .. msg, vim.log.levels.INFO) end

local function state() return require("pr").state end

--- Load fzf-lua THROUGH lz.n so its `after` hook (fzf.setup with fullscreen,
--- borders, ...) runs; a raw packadd skips setup and yields stock winopts.
local function fzf()
  local ok_lz, lzn = pcall(require, "lz.n")
  if ok_lz then pcall(lzn.trigger_load, "fzf-lua") end
  pcall(vim.cmd.packadd, "fzf-lua")
  local ok, mod = pcall(require, "fzf-lua")
  return ok and mod or nil
end

-- ---------------------------------------------------------------- columns ---

-- Display-width truncation/padding + "3d" ages live in pr.fmt (shared with
-- the pr://list buffer).
local fmt = require "pr.fmt"
local ljust, rjust, ago = fmt.ljust, fmt.rjust, fmt.ago

-- ---------------------------------------------------------------- pickers ---

function M.pr()
  local root = data.root()
  if not root then return warn "not in a git repository" end

  -- Self-serve: install the pull refspec on first use instead of nagging.
  if not data.has_refspec(root) then
    if not data.install_refspec(root) then return warn "could not write git config" end
    info "installed pull refspec for this repo"
  end

  local f = fzf()
  if not f then return warn "fzf-lua not available" end

  info "loading PRs..."
  data.prs(root, function(prs, err)
    if not prs then return warn(err or "could not list PRs") end
    if #prs == 0 then return info "no open PRs" end

    -- Columns: number | title (fills the window) | author | age, the last
    -- two right-aligned against the window edge. Fullscreen window: columns
    -- minus the single border (2), fzf's pointer gutter (2) and its
    -- scrollbar column (1) - one column over and fzf truncates the line
    -- tail into its ".." ellipsis, eating the age. Title absorbs whatever
    -- the fixed columns leave.
    local width = vim.o.columns - 5
    local num_w, author_w, age_w, gap = 6, 16, 4, 2
    local title_w = math.max(20, width - num_w - author_w - age_w - 3 * gap)

    local A = require("fzf-lua.utils").ansi_codes
    local sp = string.rep(" ", gap)
    local entries, by = {}, {}
    for _, p in ipairs(prs) do
      -- Draft is carried by the grey number alone - no [draft] column.
      local num = ("#%-5d"):format(p.number)
      local author = rjust(p.author and p.author.login or "?", author_w)
      entries[#entries + 1] = table.concat {
        p.isDraft and A.grey(num) or A.green(num),
        sp,
        ljust(p.title or "", title_w),
        sp,
        A.magenta(author),
        sp,
        A.grey(rjust(ago(p.updatedAt), age_w)),
      }
      by[p.number] = p
    end

    f.fzf_exec(entries, {
      prompt = "PR> ",
      winopts = { fullscreen = true },
      fzf_opts = { ["--ansi"] = true, ["--no-multi"] = true },
      actions = {
        -- fzf strips ANSI from selections: key on the PR number, not the line.
        ["default"] = function(sel)
          local n = sel and sel[1] and tonumber(sel[1]:match "^#(%d+)")
          local p = n and by[n]
          if p then require("pr").load(root, p) end
        end,
      },
    })
  end)
end

function M.commit()
  local S = state()
  if #S.commits == 0 then return warn "no PR loaded - <leader>gP first" end
  local f = fzf()
  if not f then return warn "fzf-lua not available" end

  -- git-log semantics: sha yellow, author blue, age grey.
  local A = require("fzf-lua.utils").ansi_codes
  local entries, by = {}, {}
  for i, c in ipairs(S.commits) do
    entries[#entries + 1] = table.concat({
      A.yellow(c.sha),
      c.subject,
      A.blue("<" .. c.author .. ">"),
      A.grey("(" .. c.ago .. ")"),
    }, "  ")
    by[c.sha] = i
  end

  -- Preview mirrors whichever mode you are in, so what you see is what loads.
  local root = vim.fn.shellescape(S.root)
  local preview = S.mode == "incremental" and ("git -C %s show --color {1}"):format(root)
    or ("git -C %s diff --color %s...{1}"):format(root, vim.fn.shellescape(S.base))

  f.fzf_exec(entries, {
    prompt = ("commit [%s]> "):format(S.mode),
    winopts = { fullscreen = true },
    fzf_opts = { ["--ansi"] = true, ["--preview"] = preview, ["--preview-window"] = "right:60%" },
    actions = {
      -- fzf strips ANSI from selections: key on the sha, not the line.
      ["default"] = function(sel)
        local i = sel and sel[1] and by[sel[1]:match "^(%x+)"]
        if i then
          state().idx = i
          require("pr").render()
        end
      end,
    },
  })
end

return M
