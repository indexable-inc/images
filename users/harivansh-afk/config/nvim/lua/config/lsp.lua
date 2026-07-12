local M = {}

local CompletionItemKind = vim.lsp.protocol.CompletionItemKind
local Methods = vim.lsp.protocol.Methods

local function ellipsize(text, max_width)
  if text == "" or vim.fn.strdisplaywidth(text) <= max_width then return text end
  return vim.fn.strcharpart(text, 0, math.max(1, max_width - 1)) .. "..."
end

local function callable_completion(kind)
  return kind == CompletionItemKind.Constructor
    or kind == CompletionItemKind.Function
    or kind == CompletionItemKind.Method
end

local function completion_abbr(item)
  local label = item.label
  if callable_completion(item.kind) then label = label:match "^[^%(]+" or label end
  return vim.trim(label)
end

local function completion_menu(item) return vim.tbl_get(item, "labelDetails", "description") or item.detail or "" end

local function completion_widths()
  local width = vim.api.nvim_win_get_width(0)
  if width < 100 then return 24, 0 end
  if width < 140 then return 32, 0 end
  return 40, 24
end

local function completion_convert(item)
  local abbr_width, menu_width = completion_widths()
  local menu = completion_menu(item)
  if menu_width > 0 then
    menu = ellipsize(menu, menu_width)
  else
    menu = ""
  end

  return {
    abbr = ellipsize(completion_abbr(item), abbr_width),
    menu = menu,
  }
end

function M.on_attach(client, bufnr)
  if client:supports_method(Methods.textDocument_completion) then
    vim.lsp.completion.enable(true, client.id, bufnr, {
      autotrigger = true,
      convert = completion_convert,
    })
  end

  local function buf(mode, lhs, rhs) bmap(mode, lhs, rhs, { buffer = bufnr }) end

  buf("n", "gd", vim.lsp.buf.definition)
  buf("n", "gD", vim.lsp.buf.declaration)
  buf("n", "<C-]>", vim.lsp.buf.definition)
  buf("n", "gi", vim.lsp.buf.implementation)
  buf("n", "gr", vim.lsp.buf.references)
  buf("n", "K", vim.lsp.buf.hover)
  buf("n", "<leader>rn", vim.lsp.buf.rename)
  buf({ "n", "v" }, "<leader>ca", vim.lsp.buf.code_action)
  buf("n", "<leader>f", function() vim.lsp.buf.format { async = true } end)
end

return M
