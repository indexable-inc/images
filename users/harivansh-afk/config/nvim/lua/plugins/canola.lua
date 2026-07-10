vim.pack.add({
  {
    src = "https://github.com/barrettruth/canola.nvim",
    version = "canola",
  },
  "https://github.com/barrettruth/canola-collection",
}, { load = function() end })

local canola_config = require "config.canola"

return {
  {
    "barrettruth/canola.nvim",
    cmd = "Canola",
    before = canola_config.setup_globals,
    after = canola_config.setup_integrations,
    keys = {
      { "-", "<cmd>Canola<cr>" },
      { "<leader>e", "<cmd>Canola<cr>" },
    },
  },
}
