local M = {}

local globals_configured = false
local integrations_configured = false

local ns = vim.api.nvim_create_namespace "canola_git_trailing"
local symbols = {
  M = { "M", "DiagnosticWarn" },
  A = { "A", "DiagnosticOk" },
  D = { "D", "DiagnosticError" },
  R = { "R", "DiagnosticWarn" },
  ["?"] = { "?", "DiagnosticInfo" },
  ["!"] = { "!", "Comment" },
}

local function apply_git_status(buf)
  if not vim.api.nvim_buf_is_valid(buf) then return end
  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)

  local ok, canola = pcall(require, "canola")
  if not ok then return end

  local dir = canola.get_current_dir(buf)
  if not dir then return end

  local git_ok, git = pcall(require, "canola-git")
  if not git_ok then return end

  local lines = vim.api.nvim_buf_line_count(buf)
  for lnum = 0, lines - 1 do
    local entry = canola.get_entry_on_line(buf, lnum + 1)
    if entry then
      local status = git.get_status(dir, entry.name)
      if status then
        local ch = status.char
        local sym = symbols[ch]
        if sym then
          vim.api.nvim_buf_set_extmark(buf, ns, lnum, 0, {
            virt_text = { { " " .. sym[1], sym[2] } },
            virt_text_pos = "eol",
            invalidate = true,
          })
        end
      end
    end
  end
end

function M.setup_globals()
  if globals_configured then return end
  globals_configured = true

  pcall(vim.cmd.packadd, "nvim-web-devicons")
  pcall(vim.cmd.packadd, "nonicons.nvim")
  vim.g.canola_git = vim.g.canola_git or {}

  vim.g.canola = {
    buf = { buflisted = false, bufhidden = "wipe" },
    columns = { "icon" },
    hidden = { enabled = false },
    highlights = { filename = {}, columns = true },
    save = "auto",
    extglob = true,
    delete = { wipe = false, recursive = true },
    float = { border = "single" },
    keymaps = {
      ["g?"] = { callback = "actions.show_help", mode = "n" },
      ["<CR>"] = "actions.select",
      ["<C-v>"] = { callback = "actions.select", opts = { vertical = true } },
      ["<C-x>"] = { callback = "actions.select", opts = { horizontal = true } },
      ["<C-p>"] = "actions.preview",
      ["<C-c>"] = { callback = "actions.close", mode = "n" },
      ["-"] = { callback = "actions.parent", mode = "n" },
      ["g."] = { callback = "actions.toggle_hidden", mode = "n" },
      ["<C-t>"] = false,
    },
  }
end

function M.setup_integrations()
  if integrations_configured then return end
  integrations_configured = true

  vim.cmd.packadd "canola-collection"

  local augroup = vim.api.nvim_create_augroup("UserCanolaConfig", { clear = true })
  local detail_columns = { "git_status", "permissions", "owner", "size", "mtime" }
  local base_columns = vim.deepcopy((vim.g.canola or {}).columns or {})
  local show_all = false

  vim.api.nvim_create_autocmd("FileType", {
    group = augroup,
    pattern = "canola",
    callback = function(args)
      local bufnr = args.buf
      local name = vim.api.nvim_buf_get_name(bufnr)
      if name:find "^fugitive://" then
        vim.schedule(function()
          if vim.api.nvim_buf_is_valid(bufnr) then
            vim.bo[bufnr].syntax = "fugitive"
            vim.bo[bufnr].filetype = "fugitive"
          end
        end)
        return
      end

      vim.api.nvim_create_autocmd("BufLeave", {
        group = augroup,
        buffer = bufnr,
        once = true,
        callback = function()
          vim.schedule(function()
            if vim.api.nvim_buf_is_valid(bufnr) and not vim.b[bufnr].canola_ready then
              vim.api.nvim_buf_delete(bufnr, { force = true })
            end
          end)
        end,
      })

      vim.keymap.set("n", "<C-f>", "<cmd>FzfLua files<cr>", { buffer = bufnr })
      vim.keymap.set("n", "<C-s>", "<cmd>FzfLua live_grep<cr>", { buffer = bufnr })

      vim.keymap.set("n", "gC", function()
        show_all = not show_all
        require("canola").set_columns(show_all and detail_columns or base_columns)
      end, {
        buffer = bufnr,
        desc = "toggle canola columns",
      })

      vim.keymap.set("n", "gX", function()
        local canola = require "canola"
        local entry = canola.get_cursor_entry()
        local dir = canola.get_current_dir()
        if not entry or not dir then return end

        vim.ui.input({ prompt = "chmod: ", default = "755" }, function(mode)
          if not mode then return end

          vim.uv.fs_chmod(dir .. entry.name, tonumber(mode, 8), function(err)
            if err then
              vim.schedule(function() vim.notify(err, vim.log.levels.ERROR) end)
              return
            end

            vim.schedule(function() require("canola.actions").refresh.callback() end)
          end)
        end)
      end, {
        buffer = bufnr,
        desc = "chmod entry",
      })
    end,
  })

  vim.api.nvim_create_autocmd("User", {
    group = augroup,
    pattern = "CanolaReadPost",
    callback = function(args)
      local buf = args.data and args.data.buf or args.buf
      if not buf then return end

      apply_git_status(buf)
      vim.defer_fn(function() apply_git_status(buf) end, 500)
    end,
  })
end

return M
