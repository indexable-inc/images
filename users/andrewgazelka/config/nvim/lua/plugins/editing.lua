-- Text editing enhancement plugins

return {
  -- Text manipulation
  { "tpope/vim-surround" },

  -- Automated bullet lists for markdown
  {
    "dkarter/bullets.vim",
    ft = { "markdown", "text", "gitcommit" },
    config = function()
      vim.g.bullets_enabled_file_types = { "markdown", "text", "gitcommit" }
      vim.g.bullets_enable_in_empty_buffers = 0
      vim.g.bullets_set_mappings = 1
      vim.g.bullets_mapping_leader = ""
      vim.g.bullets_outline_levels = { "num", "abc", "std-" }
    end,
  },

  -- nvim-autopairs for better auto pairs functionality
  {
    "windwp/nvim-autopairs",
    event = "InsertEnter",
    config = function()
      local npairs = require("nvim-autopairs")
      npairs.setup({
        disable_filetype = { "TelescopePrompt", "spectre_panel", "snacks_picker_input" },
        disable_in_macro = true, -- disable when recording or executing a macro
        disable_in_visualblock = false, -- disable when insert after visual block mode
        disable_in_replace_mode = true,
        ignored_next_char = [=[[%w%%%'%[%"%.%`%$]]=],
        enable_moveright = true,
        enable_afterquote = true, -- add bracket pairs after quote
        enable_check_bracket_line = true, --- check bracket in same line
        enable_bracket_in_quote = true,
        enable_abbr = false, -- trigger abbreviation
        break_undo = true, -- switch for basic rule break undo sequence
        check_ts = false, -- set to true if you want treesitter support
        map_cr = true,
        map_bs = true, -- map the <BS> key
        map_c_h = false, -- Map the <C-h> key to delete a pair
        map_c_w = false, -- map <c-w> to delete a pair if possible
        fast_wrap = {
          map = "<M-e>",
          chars = { "{", "[", "(", '"', "'" },
          pattern = [=[[%'%"%>%]%)%}%,]]=],
          end_key = "$",
          before_key = "h",
          after_key = "l",
          cursor_pos_before = true,
          keys = "qwertyuiopzxcvbnmasdfghjkl",
          manual_position = true,
          highlight = "Search",
          highlight_grey = "Comment",
        },
      })

      -- Add Tab functionality to jump over closing brackets like IntelliJ
      vim.keymap.set("i", "<Tab>", function()
        -- Get the current line and cursor column (0-indexed)
        local line = vim.api.nvim_get_current_line()
        local col = vim.api.nvim_win_get_cursor(0)[2]

        -- Get the next character(s)
        local next_char = line:sub(col + 1, col + 1)
        local next_two = line:sub(col + 1, col + 2)

        -- Define closing characters to jump over
        local closers = { ")", "]", "}", '"', "'", "`", ">" }

        -- Check for markdown bold (**) - jump over both asterisks
        if next_two == "**" then
          return "<Right><Right>"
        end

        -- Check for markdown italic (*) - jump over one asterisk
        if next_char == "*" then
          return "<Right>"
        end

        -- If next char is a closer, jump over it; else insert Tab
        for _, closer in ipairs(closers) do
          if next_char == closer then
            return "<Right>"
          end
        end
        return "<Tab>"
      end, { expr = true, noremap = true })
    end,
  },

  -- Multiple cursors (live updates in both normal and insert mode)
  {
    "brenton-leighton/multiple-cursors.nvim",
    version = "*",
    opts = {
      enable_split_paste = true,
    },
    keys = {
      -- Alt+n to add cursor at next match (like VS Code Ctrl+d)
      { "<M-n>", "<Cmd>MultipleCursorsAddJumpNextMatch<CR>", mode = { "n", "x" }, desc = "Add cursor at next match" },
      -- Alt+N to add cursors to ALL matches
      { "<M-N>", "<Cmd>MultipleCursorsAddMatches<CR>", mode = { "n", "x" }, desc = "Add cursors to all matches" },
      -- Alt+x to skip current and jump to next
      { "<M-x>", "<Cmd>MultipleCursorsJumpNextMatch<CR>", mode = { "n", "x" }, desc = "Skip to next match" },
      -- Add cursors above/below
      { "<C-Up>", "<Cmd>MultipleCursorsAddUp<CR>", mode = { "n", "i", "x" }, desc = "Add cursor up" },
      { "<C-Down>", "<Cmd>MultipleCursorsAddDown<CR>", mode = { "n", "i", "x" }, desc = "Add cursor down" },
      { "<up>", "<Cmd>MultipleCursorsAddUp<CR>", mode = { "n", "x" }, desc = "Add cursor up" },
      { "<down>", "<Cmd>MultipleCursorsAddDown<CR>", mode = { "n", "x" }, desc = "Add cursor down" },
      -- Ctrl+click to add/remove cursor
      { "<C-LeftMouse>", "<Cmd>MultipleCursorsMouseAddDelete<CR>", mode = { "n", "i" }, desc = "Add/remove cursor" },
      -- Lock cursors (prevent adding more)
      { "<leader>ml", "<Cmd>MultipleCursorsLock<CR>", mode = { "n", "x" }, desc = "Lock cursors" },
    },
  },
}
