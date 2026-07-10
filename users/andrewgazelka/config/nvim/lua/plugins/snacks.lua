-- Snacks.nvim for picker and various utilities

return {
  {
    "folke/snacks.nvim",
    priority = 1000,
    lazy = false,
    opts = {
      -- Disable autopairs (we use nvim-autopairs instead)
      autopairs = {
        enabled = false,
      },
      -- Enable the picker (ripgrep-based file search)
      picker = {
        enabled = true,
        -- Configure the files picker to include hidden files
        sources = {
          files = {
            hidden = true,
            -- Also show gitignored files if you want
            -- no_ignore = true,
          },
          grep = {
            hidden = true,
          },
        },
      },
      -- Enable other useful features
      bigfile = { enabled = true },
      quickfile = { enabled = true },
      statuscolumn = { enabled = true },
      words = { enabled = true },
      -- Enable dashboard with recent files
      dashboard = {
        enabled = true,
        sections = {
          { section = "keys", gap = 0, padding = 0 },
          { section = "recent_files", limit = 8, gap = 0, padding = 0 },
          { section = "projects", gap = 0, padding = 0 },
          { section = "startup" },
        },
      },
    },
    config = function(_, opts)
      local snacks = require("snacks")
      snacks.setup(opts)
    end,
  },
}
