-- File explorer (nvim-tree)

return {
  {
    "nvim-tree/nvim-tree.lua",
    dependencies = {
      "nvim-tree/nvim-web-devicons", -- optional, for file icons
    },
    config = function()
      -- Disable netrw (vim's built-in file explorer)
      vim.g.loaded_netrw = 1
      vim.g.loaded_netrwPlugin = 1

      require("nvim-tree").setup({
        -- Keep cursor on the filename (not free movement)
        hijack_cursor = true,
        -- Open tree on the left side
        view = {
          width = 35,
          side = "left",
          number = false,
          relativenumber = false,
        },
        -- Update focused file automatically
        update_focused_file = {
          enable = true,
          update_root = false,
        },
        -- Git integration
        git = {
          enable = true,
          ignore = false,
        },
        -- Show hidden files (dotfiles)
        filters = {
          dotfiles = false,
          custom = { "^.git$" }, -- Hide .git folder
        },
        -- Rendering options
        renderer = {
          icons = {
            show = {
              file = true,
              folder = true,
              folder_arrow = true,
              git = true,
            },
          },
          indent_markers = {
            enable = true,
          },
        },
        -- Actions
        actions = {
          open_file = {
            quit_on_open = false, -- Keep tree open when opening file
          },
        },
      })

      -- Keybinding: Use F13 which Ghostty will send when Cmd-1 is pressed
      vim.keymap.set("n", "<F13>", "<cmd>NvimTreeToggle<cr>", { desc = "Toggle file explorer" })

      -- Alternative keybinding with leader
      vim.keymap.set("n", "<leader>e", "<cmd>NvimTreeToggle<cr>", { desc = "Toggle file explorer" })
    end,
  },
}
