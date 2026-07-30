-- PR review flow. Lazy: nothing is required until a mapping fires.

local function pr(fn, ...)
  local args = { ... }
  return function() require("pr")[fn](unpack(args)) end
end

--- ]c / [c: commit navigation when a PR is loaded, vim's builtin diff-jump
--- in actual diff-mode windows. Buffer-local ]c maps (diffs.nvim hunk nav in
--- its own buffers) still win over this global one - deliberately.
local function step_or_builtin(delta, key)
  return function()
    if vim.wo.diff then return vim.cmd("normal! " .. key) end
    local mod = package.loaded["pr"]
    if mod and #mod.state.commits > 0 then return mod.step(delta) end
    pcall(vim.cmd, "normal! " .. key)
  end
end

--- ]p / [p: PR navigation when a pr://list is loaded, vim's builtin
--- paste-with-indent otherwise (same shape as ]c below).
local function step_pr_or_builtin(delta, key)
  return function()
    local list = package.loaded["pr.list"]
    if list and #list.order > 0 then return require("pr").step_pr(delta) end
    pcall(vim.cmd, "normal! " .. key)
  end
end

map("n", "<c-p>", pr "list", { desc = "pr: PR list" })
map("n", "]p", step_pr_or_builtin(1, "]p"), { desc = "pr: next PR / paste-indent" })
map("n", "[p", step_pr_or_builtin(-1, "[p"), { desc = "pr: prev PR / paste-indent" })
map("n", "<leader>gf", pr "files", { desc = "pr: files view" })
map("n", "<leader>gC", pr "pick_commit", { desc = "pr: pick commit in PR" })
map("n", "<leader>m", pr "toggle_mode", { desc = "pr: cumulative <-> incremental" })
map("n", "<leader>gA", pr "whole", { desc = "pr: whole PR view" })
map("n", "]c", step_or_builtin(1, "]c"), { desc = "pr: next commit / diff jump" })
map("n", "[c", step_or_builtin(-1, "[c"), { desc = "pr: prev commit / diff jump" })

--- `:e` on a pr:// buffer. Without a BufReadCmd nvim treats the name as a
--- file path, finds nothing, and leaves the buffer wiped - so the handler
--- IS the load: pr://list re-fetches the PR list, pr://files re-reads the
--- loaded PR from origin. Both adopt the current buffer (see ensure_buf),
--- which is what makes `:e pr://list` work from a cold session too.
vim.api.nvim_create_autocmd("BufReadCmd", {
  pattern = "pr://list",
  callback = function() require("pr.list").open(true) end,
})
vim.api.nvim_create_autocmd("BufReadCmd", {
  pattern = "pr://files",
  callback = function()
    local mod = require "pr"
    if #mod.state.commits == 0 then return mod.list() end -- nothing loaded: the list is the entry point
    require("pr.view").open()
    mod.reload()
  end,
})

local SUBS = {
  list = "list",
  pick = "pick_pr",
  commit = "pick_commit",
  files = "files",
  mode = "toggle_mode",
  whole = "whole",
  reload = "reload",
  refspec = "refspec",
}

--- Every verb is a subcommand too (:PR merge, :PR draft, ...), resolved out
--- of pr.verbs so the table can never fall behind the keymaps.
local function resolve(sub)
  if SUBS[sub] then return require("pr")[SUBS[sub]] end
  local verbs = require "pr.verbs"
  for _, k in ipairs(verbs.KEYS) do
    if k[2] == sub then return verbs[k[2]] end
  end
end

vim.api.nvim_create_user_command("PR", function(opts)
  local sub = opts.args ~= "" and opts.args or "list"
  local fn = resolve(sub)
  if not fn then return vim.notify("pr: unknown subcommand " .. sub, vim.log.levels.ERROR) end
  fn()
end, {
  nargs = "?",
  complete = function()
    local subs = vim.tbl_keys(SUBS)
    for _, k in ipairs(require("pr.verbs").KEYS) do
      subs[#subs + 1] = k[2]
    end
    table.sort(subs)
    return subs
  end,
  desc = "PR review flow",
})
