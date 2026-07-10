-- Colorscheme plugins

return {
  -- JetBrains theme
  {
    "nickkadutskyi/jb.nvim",
    lazy = false,
    priority = 1000,
    config = function()
      vim.cmd("colorscheme jb")
      require("config.highlights").apply_all()
    end,
  },

  -- Other colorschemes
  { "connorholyday/vim-snazzy" },
  { "catppuccin/nvim", name = "catppuccin" },
}
