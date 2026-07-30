-- pr: move between PRs, and between commits inside a PR.
--
--   <c-p>        PR list (pr://list)  ]p / [p     next / prev PR
--   :PR pick     pick a PR (fzf)      ]c / [c     next / prev commit
--   <leader>gf   files view           <leader>m   cumulative <-> incremental
--   <leader>gC   pick a commit        <leader>gA  whole-PR view
--
-- Inside either PR buffer, the verbs (pr.verbs, same keys on both):
--   D draft <-> ready    M merge    C checkout    O browser    X close
--
-- Modules:
--   pr        state + navigation + loading (this file, the orchestrator)
--   pr.data   every git/forge query, read and write
--   pr.list   the PR home surface (pr://list) - stacks render adjacent
--   pr.pick   the two fzf surfaces (PR list, commit list)
--   pr.verbs  the write side - one verb, both surfaces
--   pr.view   the files buffer - THE review surface
--
-- The files view (pr.view) is the single surface: picking a PR lands there,
-- and every navigation re-renders it. The full diffs.nvim review is reached
-- only by <CR> on a file row inside the view.
--
-- State is five values. Every action recomputes a range and re-renders.

local data = require "pr.data"

local M = {}

---@class pr.State
local S = {
  root = nil, ---@type string?
  pr = nil, ---@type table?
  base = nil, ---@type string?
  target = nil, ---@type string?
  commits = {}, ---@type table[]
  idx = 0,
  mode = "cumulative", ---@type "cumulative"|"incremental"
}

M.state = S

local function warn(msg) vim.notify("pr: " .. msg, vim.log.levels.WARN) end
local function info(msg) vim.notify("pr: " .. msg, vim.log.levels.INFO) end

--- Open (or re-render) the files view. The one and only render entry point.
function M.render()
  if #S.commits == 0 then return warn "no PR loaded - <leader>gP first" end
  require("pr.view").open()
end

M.files = M.render

-- --------------------------------------------------------------- loading ---

--- Point the whole flow at a PR (called by pr.pick with a picked entry).
function M.load(root, pr)
  local ref = data.ref(pr.number)

  local function go()
    S.root, S.pr = root, pr
    S.base = "origin/" .. pr.baseRefName
    S.target = ref
    S.commits = data.commits(root, S.base, ref)
    if #S.commits == 0 then return warn("no commits in #" .. pr.number) end
    S.idx = #S.commits -- tip: cumulative at the tip == the whole PR
    S.mode = "cumulative"
    M.render()
  end

  if data.ref_exists(root, ref) then return go() end

  info("fetching #" .. pr.number .. "...")
  data.fetch(root, function(ok, err)
    if not ok then return warn("fetch failed: " .. (err or "")) end
    if not data.ref_exists(root, ref) then
      return warn("no local ref " .. ref .. " - is the pull refspec installed? :PR refspec")
    end
    go()
  end)
end

--- Re-read the loaded PR from the remote: what `:e` and `R` mean on
--- pr://files. `:e` on a real file rereads it from disk, so on a PR buffer
--- it rereads the PR from origin - a force-push or a new commit lands here.
---
--- Renders from cache FIRST and fetches behind it: :e must never leave a
--- blank buffer sitting there while the network answers.
function M.reload()
  if #S.commits == 0 then return warn "no PR loaded - <c-p> first" end
  M.render()
  local at = S.commits[S.idx] and S.commits[S.idx].sha
  info("refreshing #" .. S.pr.number .. "...")
  data.fetch(S.root, function(ok, err) -- clears every diff cache on the way
    if not ok then return warn("fetch failed: " .. (err or "")) end
    local commits = data.commits(S.root, S.base, S.target)
    if #commits == 0 then return warn("no commits in #" .. S.pr.number) end
    S.commits = commits
    -- Hold position on the same commit; a force-push that rewrote it drops
    -- us at the tip, which is where a rewritten PR wants reviewing anyway.
    S.idx = #commits
    for i, c in ipairs(commits) do
      if c.sha == at then S.idx = i end
    end
    M.render()
  end)
end

-- --------------------------------------------------------------- pickers ---

function M.list() require("pr.list").open() end

function M.pick_pr() require("pr.pick").pr() end

function M.pick_commit() require("pr.pick").commit() end

-- ------------------------------------------------------------- navigation ---

---@param delta integer
function M.step(delta)
  if #S.commits == 0 then return warn "no PR loaded - <leader>gP first" end
  local n = #S.commits
  S.idx = ((S.idx - 1 + delta) % n) + 1
  M.render()
end

--- ]p / [p: step to the adjacent PR in pr://list display order (stacks are
--- adjacent there, so this walks a stack parent-first). Wraps like M.step.
---@param delta integer
function M.step_pr(delta)
  local list = require "pr.list"
  local n = #list.order
  if n == 0 or not list.root then return warn "no PR list - <c-p> first" end
  local cur = 0
  for i, p in ipairs(list.order) do
    if S.pr and p.number == S.pr.number then
      cur = i
      break
    end
  end
  -- No current PR: ]p enters at the top, [p at the bottom.
  local nxt = cur == 0 and (delta > 0 and 1 or n) or ((cur - 1 + delta) % n) + 1
  M.load(list.root, list.order[nxt])
end

function M.toggle_mode()
  if #S.commits == 0 then return warn "no PR loaded - <leader>gP first" end
  S.mode = S.mode == "cumulative" and "incremental" or "cumulative"
  M.render()
end

--- Jump back to the whole-PR view (cumulative at the tip).
function M.whole()
  if #S.commits == 0 then return warn "no PR loaded - <leader>gP first" end
  S.idx, S.mode = #S.commits, "cumulative"
  M.render()
end

-- ------------------------------------------------------------------ setup ---

function M.refspec()
  local root = data.root()
  if not root then return warn "not in a git repository" end
  if data.has_refspec(root) then return info "pull refspec already installed" end
  if not data.install_refspec(root) then return warn "could not write git config" end
  info "refspec installed, fetching PR refs..."
  data.fetch(root, function(ok, err)
    if ok then
      info "PR refs available - <leader>gP to pick one"
    else
      warn("fetch failed: " .. (err or ""))
    end
  end)
end

return M
