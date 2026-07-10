vim.pack.add({
  "https://github.com/lewis6991/gitsigns.nvim",
  "https://github.com/barrettruth/forge.nvim",
  "https://github.com/barrettruth/diffs.nvim",
}, { load = function() end })

return {
  {
    "lewis6991/gitsigns.nvim",
    event = "BufReadPre",
    after = function()
      require("gitsigns").setup {
        base = "HEAD",
        signs = {
          add = { text = "██" },
          change = { text = "██" },
          delete = { text = "▄▄" },
          topdelete = { text = "▀▀" },
          changedelete = { text = "██" },
        },
        signs_staged = {
          add = { text = "▓▓" },
          change = { text = "▓▓" },
          delete = { text = "▄▄" },
          topdelete = { text = "▀▀" },
          changedelete = { text = "▓▓" },
        },
        signs_staged_enable = true,
      }

      map("n", "]g", "<cmd>Gitsigns next_hunk<cr>")
      map("n", "[g", "<cmd>Gitsigns prev_hunk<cr>")
      map("n", "<leader>ghs", "<cmd>Gitsigns stage_hunk<cr>")
      map("n", "<leader>ghr", "<cmd>Gitsigns reset_hunk<cr>")
      map("n", "<leader>ghp", "<cmd>Gitsigns preview_hunk<cr>")
      map("n", "<leader>gB", "<cmd>Gitsigns toggle_current_line_blame<cr>")
    end,
  },
  {
    "barrettruth/forge.nvim",
    cmd = "Forge",
    before = function()
      vim.g.forge = {
        sources = {
          forgejo = { hosts = { "git.harivan.sh" } },
        },
      }
    end,
    after = function() pcall(vim.cmd.packadd, "fzf-lua") end,
    keys = {
      { "<c-t>", [[<cmd>lua require('forge').open()<cr>]], desc = "forge" },
    },
  },
  {
    "barrettruth/diffs.nvim",
    before = function()
      vim.g.diffs = {
        integrations = {
          fugitive = true,
        },
        view = {
          prefix = false,
        },
        highlights = {
          warn_max_lines = false,
          blend_alpha = 0.5,
          intra = { enabled = true },
        },
      }
    end,
  },
}
