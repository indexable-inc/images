-- UI enhancement plugins

return {
  -- Progress indicator - minimal bottom-right notifications
  {
    "j-hui/fidget.nvim",
    opts = {
      notification = {
        window = {
          winblend = 0,
          relative = "editor",
          align = "bottom",
          x_padding = 1,
          y_padding = 0,
        },
      },
      progress = {
        display = {
          done_icon = "✓",
          progress_icon = { pattern = "dots", period = 1 },
        },
      },
    },
  },

  -- Modern folding with LSP/treesitter support
  {
    "kevinhwang91/nvim-ufo",
    dependencies = { "kevinhwang91/promise-async" },
    config = function()
      require("ufo").setup({
        provider_selector = function(bufnr, filetype, buftype)
          -- Use treesitter for markdown to get header-based folding
          if filetype == "markdown" then
            return { "treesitter", "indent" }
          end
          -- Use LSP first, fallback to indent for other files
          return { "lsp", "indent" }
        end,
        preview = {
          win_config = {
            winblend = 0,  -- Disable transparency animations
          },
        },
        -- Disable fold opening animations
        open_fold_hl_timeout = 0,
      })

      -- Keymaps for folding
      vim.keymap.set("n", "zR", require("ufo").openAllFolds, { desc = "Open all folds" })
      vim.keymap.set("n", "zM", require("ufo").closeAllFolds, { desc = "Close all folds" })
      vim.keymap.set("n", "zr", require("ufo").openFoldsExceptKinds, { desc = "Fold less" })
      vim.keymap.set("n", "zm", require("ufo").closeFoldsWith, { desc = "Fold more" })
      vim.keymap.set("n", "zp", function()
        local winid = require("ufo").peekFoldedLinesUnderCursor()
        if not winid then
          vim.lsp.buf.hover()
        end
      end, { desc = "Peek fold" })
    end,
  },

  -- Sticky scroll - show context at top of window (like VSCode)
  {
    "nvim-treesitter/nvim-treesitter-context",
    dependencies = { "nvim-treesitter/nvim-treesitter" },
    opts = {
      enable = true,
      max_lines = 3, -- How many lines the window should span
      min_window_height = 20, -- Minimum editor window height to enable context
      trim_scope = "outer", -- Which context lines to discard if max_lines exceeded
      mode = "cursor", -- Line used to calculate context. Choices: 'cursor', 'topline'
      separator = nil, -- Separator between context and content. nil uses no separator
    },
  },
}
