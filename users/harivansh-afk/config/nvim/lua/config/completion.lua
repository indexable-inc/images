local M = {}

local function context(base)
  local cursor = vim.api.nvim_win_get_cursor(0)
  local row = cursor[1]
  local col = cursor[2]
  local line = vim.api.nvim_get_current_line()

  return {
    base = base,
    before = line:sub(1, col),
    bufnr = vim.api.nvim_get_current_buf(),
    col = col,
    filetype = vim.bo.filetype,
    line = line,
    row = row,
  }
end

local function env_state(ctx)
  local query = type(ctx.base) == "string" and ctx.base or ""
  local base = ctx.before:match "%$([%w_]*)$"
  if base ~= nil then
    return {
      base = query ~= "" and query or base,
      close_brace = false,
      start = ctx.col - #base,
    }
  end

  base = ctx.before:match("%$" .. "{([%w_]*)$")
  if base ~= nil then
    return {
      base = query ~= "" and query or base,
      close_brace = true,
      start = ctx.col - #base,
    }
  end
end

local function item_info(value)
  local text = tostring(value):gsub("%s+", " ")
  if #text > 120 then text = text:sub(1, 117) .. "..." end
  return text
end

local function item_abbr(close_brace, name)
  if close_brace then return "$" .. "{" .. name .. "}" end
  return "$" .. name
end

local function complete_env(st)
  local env = vim.fn.environ()
  local names = {}

  for name in pairs(env) do
    names[#names + 1] = name
  end

  if st.base == "" then
    table.sort(names, function(a, b) return a:lower() < b:lower() end)
  else
    names = vim.fn.matchfuzzy(names, st.base, { matchseq = 1 })
  end

  local words = {}
  for _, name in ipairs(names) do
    words[#words + 1] = {
      word = name,
      abbr = item_abbr(st.close_brace, name),
      icase = 1,
      info = item_info(env[name]),
      menu = "[env]",
      user_data = {
        env = {
          close_brace = st.close_brace,
        },
        source = "env",
      },
    }
  end

  return words
end

function M.complete(findstart, base)
  local ctx = context(base)
  local st = env_state(ctx)

  if findstart == 1 then return st and st.start or -2 end
  if not st then return { refresh = "always", words = {} } end

  return {
    refresh = "always",
    words = complete_env(st),
  }
end

function M.on_complete_done()
  local item = vim.v.completed_item
  if type(item) ~= "table" then return end
  if not vim.tbl_get(item, "user_data", "env", "close_brace") then return end

  local word = item.word
  if type(word) ~= "string" or word == "" then return end

  local cursor = vim.api.nvim_win_get_cursor(0)
  local row = cursor[1]
  local col = cursor[2]
  local line = vim.api.nvim_get_current_line()
  local insert_col

  for _, candidate in ipairs { col, col + 1 } do
    if candidate >= #word then
      local start = candidate - #word + 1
      if line:sub(start, candidate) == word then
        insert_col = candidate
        break
      end
    end
  end

  if not insert_col then return end
  if line:sub(insert_col + 1, insert_col + 1) == "}" then return end

  vim.api.nvim_buf_set_text(0, row - 1, insert_col, row - 1, insert_col, { "}" })
end

vim.api.nvim_create_autocmd("CompleteDone", {
  group = vim.api.nvim_create_augroup("native_completion", { clear = true }),
  callback = M.on_complete_done,
})

return M
