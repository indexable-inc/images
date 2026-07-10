vim.pack.add {
  "https://github.com/tpope/vim-fugitive",
}

map("n", "<C-g>", "<cmd>Git<cr><cmd>only<cr>")
map("n", "<leader>gg", "<cmd>Git<cr><cmd>only<cr>")
map("n", "<leader>gc", "<cmd>Git commit<cr>")
map("n", "<leader>gp", "<cmd>Git push<cr>")
map("n", "<leader>gl", "<cmd>Git pull<cr>")
map("n", "<leader>gb", "<cmd>Git blame<cr>")
map("n", "<leader>gd", "<cmd>Gvdiffsplit<cr>")
map("n", "<leader>gr", "<cmd>Gread<cr>")
map("n", "<leader>gw", "<cmd>Gwrite<cr>")
