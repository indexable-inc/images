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

-- Right-aligned "+N -N" LOC per file row, same shape and colors as the
-- pr://files view (one definition, lua/gitstats.lua). FugitiveIndex is
-- fired by fugitive#BufReadStatus at the end of EVERY status render -
-- first load and every reload - once the buffer lines are final, so it is
-- the exact hook for decorating without touching fugitive's own rendering.
vim.api.nvim_create_autocmd("User", {
  pattern = "FugitiveIndex",
  group = vim.api.nvim_create_augroup("gitstats_fugitive", { clear = true }),
  callback = function(ev) require("gitstats").fugitive(ev.buf) end,
})
