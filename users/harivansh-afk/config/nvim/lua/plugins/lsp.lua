vim.pack.add({
  "https://github.com/neovim/nvim-lspconfig",
}, { load = function() end })

return {
  {
    "neovim/nvim-lspconfig",
    after = function()
      local elixir_ls = vim.fn.exepath "elixir-ls"
      if elixir_ls ~= "" then
        vim.lsp.config("elixirls", {
          cmd = { elixir_ls },
          filetypes = { "elixir", "eelixir", "heex", "surface" },
          root_markers = { "mix.exs", ".git" },
          settings = {
            elixirLS = {
              dialyzerEnabled = true,
              fetchDeps = false,
            },
          },
        })
        vim.lsp.enable "elixirls"
      end
    end,
  },
}
