-- pr.fmt: small presentation helpers shared by the pr surfaces.
--
-- Truncation and padding go by DISPLAY width, not bytes - "…" is 3 bytes but
-- 1 cell, and byte-based %-Ns padding scatters every column after a
-- multibyte char.

local M = {}

local ns = vim.api.nvim_create_namespace "pr_fmt"

function M.trunc(str, w)
  if vim.fn.strdisplaywidth(str) > w then
    while vim.fn.strdisplaywidth(str) > w - 1 do
      str = vim.fn.strcharpart(str, 0, vim.fn.strchars(str) - 1)
    end
    str = str .. "…"
  end
  return str
end

function M.ljust(str, w)
  str = M.trunc(str, w)
  return str .. string.rep(" ", w - vim.fn.strdisplaywidth(str))
end

function M.rjust(str, w)
  str = M.trunc(str, w)
  return string.rep(" ", w - vim.fn.strdisplaywidth(str)) .. str
end

--- "3d" from an ISO-8601 UTC timestamp (both forges emit Z). There is no
--- portable "UTC fields -> epoch" in Lua, so both sides go through os.time,
--- which reads its table as LOCAL time - the shift cancels only if BOTH
--- sides get the same offset.
---
--- isdst=false is what makes that true. os.date "!*t" sets isdst=false
--- itself; leaving it off the parsed table means tm_isdst=-1, so mktime
--- GUESSES DST for that date and the two sides differ by exactly one hour
--- whenever local time is on summer time - a PR opened 3 minutes ago
--- measured 3784s and rendered "1h". Pinning both to standard time makes
--- the offsets identical and the subtraction exact, year-round.
function M.ago(iso)
  local y, mo, d, h, mi, sec = (iso or ""):match "(%d+)-(%d+)-(%d+)T(%d+):(%d+):(%d+)"
  if not y then return "" end
  local t = os.time { year = y, month = mo, day = d, hour = h, min = mi, sec = sec, isdst = false }
  local dt = os.time(os.date "!*t") - t
  if dt < 3600 then return math.max(1, math.floor(dt / 60)) .. "m" end
  if dt < 86400 then return math.floor(dt / 3600) .. "h" end
  if dt < 86400 * 30 then return math.floor(dt / 86400) .. "d" end
  return math.floor(dt / 86400 / 30) .. "mo"
end

--- g? - the fugitive gesture: a small float over the cursor, keys
--- highlighted in fugitiveHelpTag like fugitive's own help column.
---@param title string
---@param entries {[1]:string,[2]:string}[] key, description
function M.help(title, entries)
  local keyw, width = 0, 0
  for _, h in ipairs(entries) do
    keyw = math.max(keyw, #h[1])
  end
  local lines = {}
  for _, h in ipairs(entries) do
    lines[#lines + 1] = (" %-" .. keyw .. "s  %s"):format(h[1], h[2])
    width = math.max(width, #lines[#lines])
  end
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, lines)
  vim.bo[b].modifiable = false
  vim.bo[b].bufhidden = "wipe"
  for i, h in ipairs(entries) do
    vim.api.nvim_buf_set_extmark(b, ns, i - 1, 1, { end_col = 1 + #h[1], hl_group = "fugitiveHelpTag" })
  end
  local win = vim.api.nvim_open_win(b, true, {
    relative = "cursor",
    row = 1,
    col = 0,
    width = width + 1,
    height = #lines,
    style = "minimal",
    border = "single",
    title = title,
    title_pos = "center",
  })
  for _, k in ipairs { "q", "<Esc>", "g?" } do
    vim.keymap.set("n", k, "<cmd>close<cr>", { buffer = b, silent = true, nowait = true })
  end
  vim.api.nvim_create_autocmd("BufLeave", {
    buffer = b,
    once = true,
    callback = function()
      if vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
    end,
  })
end

return M
