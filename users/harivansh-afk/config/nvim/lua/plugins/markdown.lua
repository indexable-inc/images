vim.pack.add({
  "https://github.com/MeanderingProgrammer/render-markdown.nvim",
}, { load = function() end })

return {
  {
    "MeanderingProgrammer/render-markdown.nvim",
    ft = { "markdown", "codecompanion" },
    after = function()
      require("render-markdown").setup {
        completions = { lsp = { enabled = true } },
      }
    end,
    keys = {
      { "<leader>tm", "<cmd>RenderMarkdown toggle<cr>", mode = "n", desc = "Toggle render-markdown" },
    },
  },
}
