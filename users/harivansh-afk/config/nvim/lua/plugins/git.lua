vim.pack.add({
  "https://github.com/lewis6991/gitsigns.nvim",
  -- barrettruth deleted his forge.nvim; the public fork is the source now.
  -- Upstream gone means GitHub answers the anonymous clone with an auth
  -- prompt, which kills vim.pack on any box doing a fresh install.
  "https://github.com/harivansh-afk/forge.nvim",
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
    "harivansh-afk/forge.nvim",
    cmd = "Forge",
    before = function()
      vim.g.forge = {
        sources = {
          -- Key must name a module under lua/forge/. Forgejo/Gitea are served
          -- by codeberg.lua (tea CLI); `forgejo` is only a host substring, and
          -- using it as the key makes detection silently return nil.
          codeberg = { hosts = { "git.harivan.sh" } },
        },
      }
    end,
    after = function() pcall(vim.cmd.packadd, "fzf-lua") end,
    keys = {
      -- forge has no M.open(); the entry point is the pickers module.
      { "<c-t>", [[<cmd>lua require('forge.pickers').git()<cr>]], desc = "forge" },
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
