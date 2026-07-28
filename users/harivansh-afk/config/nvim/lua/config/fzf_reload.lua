local M = {}
M.opts = nil

function M.setup(opts) M.opts = vim.deepcopy(opts or {}) end

function M.reload()
  local path = vim.env.FZF_DEFAULT_OPTS_FILE or vim.fn.expand "~/.config/fzf/themes/theme"
  if vim.fn.filereadable(path) == 0 then return end

  local lines = vim.fn.readfile(path)
  if not lines or #lines == 0 or not M.opts then return end

  local colors = {}
  for color_spec in table.concat(lines, "\n"):gmatch "%-%-color=([^%s]+)" do
    for key, value in color_spec:gmatch "([^:,]+):([^,]+)" do
      colors[key] = value
    end
  end

  M.opts.fzf_colors = colors
  require("fzf-lua").setup(M.opts)
end

return M
