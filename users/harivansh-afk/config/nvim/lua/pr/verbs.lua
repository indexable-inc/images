-- pr.verbs: the write side of the flow - the things that CHANGE a PR.
--
--   D  draft <-> ready     M  merge (escalates to force on refusal)
--   O  open in browser     C  check out the branch
--   X  close
--
-- Capitals on purpose: every one of these letters is a no-op in a
-- non-modifiable buffer (D/C are line operators, O/X/A insert-or-delete),
-- so nothing useful is shadowed - and the review keys stay lowercase.
--
-- One verb works on BOTH surfaces. pr://list acts on the cursor line,
-- pr://files on the loaded PR, and since both hand back the SAME PR table
-- (pr.list.order entries are what pr.load stores in state.pr), a verb that
-- flips pr.isDraft is seen by every surface at once - no re-fetch to show
-- the new state.
--
-- Prompts are vim.fn.confirm, deliberately: it is the one picker that draws
-- in the cmdline at the bottom of the screen, right under the error that
-- provoked it. fzf and vim.ui.select float away over the diff.

local data = require "pr.data"

local M = {}

local function warn(msg) vim.notify("pr: " .. msg, vim.log.levels.WARN) end
local function info(msg) vim.notify("pr: " .. msg, vim.log.levels.INFO) end

--- The PR a verb acts on, plus its repo root.
---@return string? root, table? pr
local function target()
  local list = package.loaded["pr.list"]
  if list and vim.api.nvim_buf_get_name(0):match "pr://list$" then
    local p = list.order[vim.fn.line "."]
    if p and list.root then return list.root, p end
  end
  local s = require("pr").state
  if s.pr and s.root then return s.root, s.pr end
end

--- Re-paint whatever surfaces are live, without touching the network. Used
--- after a verb mutates the PR table in place.
local function repaint()
  local list = package.loaded["pr.list"]
  if list then list.repaint() end
  local view = package.loaded["pr.view"]
  if view and view.active() then view.render() end
end

--- The PR just left the open set (merged or closed). Its row must go, and
--- the stack shape around it changes, so this is a real re-fetch. From
--- either PR surface that means landing back on the list - the diff below
--- is of something that no longer needs reviewing.
local function gone()
  local name = vim.api.nvim_buf_get_name(0)
  if name:match "pr://" then return require("pr.list").open(true) end
  require("pr.list").invalidate() -- :PR merge from elsewhere: refetch on next open
end

---@return boolean
local function yes(msg) return vim.fn.confirm(msg, "&yes\n&no", 2) == 1 end

-- ------------------------------------------------------------------ draft ---

--- One key, both directions - the current state decides which. No confirm:
--- the same key undoes it.
function M.draft()
  local root, p = target()
  if not p then return warn "no PR here" end
  local to = not p.isDraft
  info(("#%d -> %s..."):format(p.number, to and "draft" or "ready"))
  data.set_draft(root, p, to, function(ok, err)
    if not ok then return warn((to and "draft" or "ready") .. " failed: " .. (err or "")) end
    p.isDraft = to
    info(("#%d is %s"):format(p.number, to and "draft" or "ready"))
    repaint()
  end)
end

-- ------------------------------------------------------------------ merge ---

local STYLES = { "squash", "merge", "rebase" }

--- Ask only what the repo actually allows (pr.data.merge_policy):
---   nothing known -> plain y/n, forge picks the style (tea)
---   one style     -> y/n, naming it
---   several       -> s/m/r, pre-selecting the forge's own default
--- confirm returns 0 on <Esc> and #allowed+1 on "quit", both of which index
--- past the list and abort.
---@return string? style  "" = let the forge decide; nil = cancelled
local function choose_style(p, policy)
  local allowed = policy.allowed
  if #allowed == 0 then return yes(("merge #%d?"):format(p.number)) and "" or nil end
  if #allowed == 1 then return yes(("merge #%d (%s)?"):format(p.number, allowed[1])) and allowed[1] or nil end
  local labels, default = {}, 1
  for i, s in ipairs(allowed) do
    labels[i] = "&" .. s
    if s == policy.default then default = i end
  end
  labels[#labels + 1] = "&quit"
  return allowed[vim.fn.confirm(("merge #%d?"):format(p.number), table.concat(labels, "\n"), default)]
end

--- Retry menu after tea refuses a merge. tea has no --admin, and a Gitea
--- refusal is most often "this repo does not allow that merge style", so
--- the escalation there is a different STYLE rather than more privilege.
---@return string? style
local function choose_retry(msg)
  local c = vim.fn.confirm(msg .. "retry with another style?", "&squash\n&merge\n&rebase\n&quit", 4)
  return STYLES[c]
end

local function do_merge(root, p, style, force)
  info(("merging #%d..."):format(p.number))
  data.merge(root, p, { style = style ~= "" and style or nil, force = force }, function(ok, err)
    if ok then
      info(("#%d merged"):format(p.number))
      return gone()
    end
    -- The escalation prompt carries the refusal with it, so the reason and
    -- the y/n sit in the cmdline together.
    local msg = ("merge #%d failed:\n%s\n\n"):format(p.number, err or "?")
    if force then return warn(msg) end -- already escalated: nothing left to offer
    if data.can_force(root) then
      if yes(msg .. "force merge (--admin)?") then do_merge(root, p, style, true) end
      return
    end
    local retry = choose_retry(msg)
    if retry then do_merge(root, p, retry, false) end
  end)
end

function M.merge()
  local root, p = target()
  if not p then return warn "no PR here" end
  if p.isDraft and not yes(("#%d is a draft. merge anyway?"):format(p.number)) then return end
  data.merge_policy(root, function(policy)
    local style = choose_style(p, policy)
    if style then do_merge(root, p, style, false) end
  end)
end

-- ------------------------------------------------------- close / checkout ---

function M.close()
  local root, p = target()
  if not p then return warn "no PR here" end
  if not yes(("close #%d without merging?"):format(p.number)) then return end
  data.close(root, p, function(ok, err)
    if not ok then return warn("close failed: " .. (err or "")) end
    info(("#%d closed"):format(p.number))
    gone()
  end)
end

--- Check out the PR branch into the working tree. Fires FugitiveChanged so
--- the statusline / fugitive buffers notice the branch moved under them.
function M.checkout()
  local root, p = target()
  if not p then return warn "no PR here" end
  info(("checking out #%d..."):format(p.number))
  data.checkout(root, p, function(ok, err)
    if not ok then return warn("checkout failed: " .. (err or "")) end
    info(("#%d checked out%s"):format(p.number, p.headRefName and (" (" .. p.headRefName .. ")") or ""))
    pcall(vim.cmd, "silent! doautocmd User FugitiveChanged")
  end)
end

function M.web()
  local root, p = target()
  if not p then return warn "no PR here" end
  local url = data.web_url(root, p.number)
  if not url then return warn "could not derive a URL from origin" end
  local ok, err = pcall(vim.ui.open, url)
  if not ok then return warn("open failed: " .. tostring(err)) end
  info(url)
end

-- ------------------------------------------------------------------- maps ---

--- The verb keys, shared verbatim by pr://list and pr://files so a verb is
--- the same keystroke wherever you are.
M.KEYS = {
  { "D", "draft", "draft <-> ready" },
  { "M", "merge", "merge (force on refusal)" },
  { "C", "checkout", "check out the branch" },
  { "O", "web", "open in browser" },
  { "X", "close", "close without merging" },
}

---@param buf integer
function M.attach(buf)
  for _, k in ipairs(M.KEYS) do
    vim.keymap.set("n", k[1], function() M[k[2]]() end, { buffer = buf, silent = true, desc = "pr: " .. k[3] })
  end
end

--- The verb rows for a g? help float.
---@return {[1]:string,[2]:string}[]
function M.help_entries()
  local out = {}
  for _, k in ipairs(M.KEYS) do
    out[#out + 1] = { k[1], k[3] }
  end
  return out
end

return M
