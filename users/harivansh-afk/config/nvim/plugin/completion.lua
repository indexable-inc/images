vim.opt.complete = { "o", ".", "w", "b" }
vim.o.completefunc = "v:lua.require'config.completion'.complete"
vim.opt.completeopt = { "menuone", "noinsert", "popup" }

vim.o.autocomplete = true
vim.o.pumborder = "single"
vim.o.pumheight = 15
vim.o.pumwidth = 24
vim.o.pummaxwidth = 80

local function current_preview_winid()
  local info = vim.fn.complete_info { "preview_winid" }
  return info.preview_winid or 0
end

local function scroll_preview(direction)
  local winid = current_preview_winid()
  if winid == 0 or not vim.api.nvim_win_is_valid(winid) then return false end

  local height = vim.api.nvim_win_get_height(winid)
  local step = math.max(1, math.floor(height / 2))
  local bufnr = vim.api.nvim_win_get_buf(winid)
  local last_line = vim.api.nvim_buf_line_count(bufnr)
  local delta = direction == "down" and step or -step
  local current_line = vim.api.nvim_win_get_cursor(winid)[1]
  local line = math.min(last_line, math.max(1, current_line + delta))
  vim.api.nvim_win_set_cursor(winid, { line, 0 })
  return true
end

local function semantic_completion()
  local prefix = vim.fn.pumvisible() == 1 and "<c-e>" or ""
  if vim.bo.omnifunc ~= "" then return prefix .. "<c-x><c-o>" end
  return prefix .. "<c-n>"
end

local function completion_or_preview(keys, direction)
  if scroll_preview(direction) then return "" end
  if vim.fn.pumvisible() == 1 then return "" end
  return keys
end

local function snippet_jump(direction)
  if not vim.snippet.active { direction = direction } then return nil end
  return ("<cmd>lua vim.snippet.jump(%d)<cr>"):format(direction)
end

vim.keymap.set(
  "i",
  "<c-b>",
  function() return completion_or_preview("<c-x><c-n>", "up") end,
  { expr = true, desc = "buffer completion or docs backward" }
)

vim.keymap.set(
  "i",
  "<c-e>",
  function() return vim.fn.pumvisible() == 1 and "<c-e>" or "<c-x><c-u>" end,
  { expr = true, desc = "env completion or cancel completion" }
)

vim.keymap.set(
  "i",
  "<c-f>",
  function() return completion_or_preview("<c-x><c-f>", "down") end,
  { expr = true, desc = "file completion or docs forward" }
)

vim.keymap.set(
  "i",
  "<c-n>",
  function() return vim.fn.pumvisible() == 1 and "<c-n>" or semantic_completion() end,
  { expr = true, desc = "semantic completion or next completion" }
)

vim.keymap.set("i", "<c-p>", function() return "<c-p>" end, { expr = true, desc = "previous completion" })

vim.keymap.set("i", "<c-s>", semantic_completion, { expr = true, desc = "semantic completion" })

vim.keymap.set({ "i", "s" }, "<tab>", function()
  if vim.fn.pumvisible() == 1 then
    -- 'autocomplete' forces noselect, so nothing is selected when the menu
    -- opens; select the first item before <c-y> can accept anything.
    if vim.fn.complete_info({ "selected" }).selected == -1 then return "<c-n>" end
    return "<c-y>"
  end
  return snippet_jump(1) or "<tab>"
end, { expr = true, desc = "select or accept completion, or jump snippet" })

vim.keymap.set({ "i", "s" }, "<s-tab>", function()
  if vim.fn.pumvisible() == 1 then return "<c-p>" end
  return snippet_jump(-1) or "<s-tab>"
end, { expr = true, desc = "previous completion or jump snippet backward" })
