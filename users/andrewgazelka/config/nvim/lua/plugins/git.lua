-- Git-related plugins

return {
  -- Git signs in the gutter
  {
    "lewis6991/gitsigns.nvim",
    config = function()
      local gitsigns = require('gitsigns')
      gitsigns.setup({
        signs = {
          add          = { text = '▎' },
          change       = { text = '▎' },
          delete       = { text = '▎' },
          topdelete    = { text = '▎' },
          changedelete = { text = '▎' },
          untracked    = { text = '▎' },
        },
        -- Disable line number highlighting
        numhl = false,
        -- Highlight the sign column
        signcolumn = true,
        -- Disable line highlighting
        linehl = false,
        -- Disable inline blame
        current_line_blame = false,
        -- Performance optimizations
        watch_gitdir = {
          follow_files = false,  -- Don't track file moves (faster)
        },
        update_debounce = 200,  -- Debounce updates by 200ms (reduces git checks on rapid saves)
        max_file_length = 10000,  -- Skip gitsigns for very large files
      })

      -- Keybindings for git
      vim.keymap.set('n', '<leader>gb', ':Gitsigns toggle_linehl<CR>', { desc = 'Toggle line highlight heatmap' })
      vim.keymap.set('n', '<leader>gd', gitsigns.diffthis, { desc = 'Git diff' })
    end,
  },

  -- Git commands
  {
    "tpope/vim-fugitive",
    config = function()
      -- Open git blame in vertical split with colors
      vim.keymap.set("n", "<leader>gB", ":Git blame<CR>", { desc = "Open git blame sidebar" })
    end,
  },

  -- Git blame inline (current line only)
  {
    "f-person/git-blame.nvim",
    config = function()
      vim.g.gitblame_enabled = 0  -- Start disabled, toggle with <leader>gt
      vim.g.gitblame_date_format = '%r'
      vim.g.gitblame_message_template = '<author> • <date> • <summary>'
      vim.g.gitblame_ignored_filetypes = { 'markdown', 'typst' }

      vim.keymap.set("n", "<leader>gt", ":GitBlameToggle<CR>", { desc = "Toggle git blame" })
      vim.keymap.set("n", "<leader>go", ":GitBlameOpenCommitURL<CR>", { desc = "Open commit in browser" })
    end,
  },
}
