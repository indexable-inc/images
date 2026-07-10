-- Motion and navigation plugins

return {
  -- Flash (motion)
  {
    "folke/flash.nvim",
    event = "VeryLazy",
    opts = {},
    keys = {
      { "f", mode = { "n", "x", "o" } },
      { "F", mode = { "n", "x", "o" } },
      { "t", mode = { "n", "x", "o" } },
      { "T", mode = { "n", "x", "o" } },
      { "<leader><leader>s", mode = { "n", "x", "o" }, function() require("flash").jump() end, desc = "Flash" },
      { "<leader><leader>w", mode = { "n", "x", "o" }, function() require("flash").jump({ search = { mode = "search", max_length = 0 }, label = { after = { 0, 0 } }, pattern = "^" }) end, desc = "Flash line start" },
    },
    config = function()
      require('flash').setup({
        labels = "asdfghjklqwertyuiopzxcvbnm",
        search = {
            multi_window = true,
            forward = true,
            wrap = true,
        },
        jump = {
            jumplist = true,
            pos = "start",
            history = false,
            register = false,
            nohlsearch = false,
            autojump = false,
        },
        label = {
            uppercase = true,
            exclude = "",
            current = true,
            after = true,
            before = false,
            style = "overlay",
            reuse = "lowercase",
            distance = true,
        },
        highlight = {
            backdrop = false,  -- No background graying
            matches = true,
            priority = 5000,
            groups = {
                match = "FlashMatch",
                current = "FlashCurrent",
                backdrop = "FlashBackdrop",
                label = "FlashLabel",
            },
        },
        modes = {
            search = {
                enabled = false,  -- Disable Flash search mode to keep normal vim search
                highlight = { backdrop = false },
                jump = { history = true, register = true, nohlsearch = true },
            },
            char = {
                enabled = true,
                config = function(opts)
                    -- Enable jump labels always for f/F/t/T
                    opts.jump_labels = true
                    return opts
                end,
                autohide = false,
                jump_labels = true,
                multi_line = true,
                label = { exclude = "" },
                keys = { "f", "F", "t", "T", ";", "," },
                char_actions = function(motion)
                    return {
                        [";"] = "next",
                        [","] = "prev",
                        [motion:lower()] = "next",
                        [motion:upper()] = "prev",
                    }
                end,
                search = { wrap = false },
                highlight = { backdrop = false },
                jump = { register = false },
            },
            treesitter = {
                labels = "abcdefghijklmnopqrstuvwxyz",
                jump = { pos = "range" },
                search = { incremental = false },
                label = { before = true, after = true, style = "inline" },
                highlight = {
                    backdrop = false,
                    matches = false,
                },
            },
            treesitter_search = {
                jump = { pos = "range" },
                search = { multi_window = true, wrap = true, incremental = false },
                remote_op = { restore = true },
                label = { before = false, after = true, style = "inline" },
            },
            remote = {
                remote_op = { restore = true, motion = true },
            },
        },
      })
    end,
  },
}
