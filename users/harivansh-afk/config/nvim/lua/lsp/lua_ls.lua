return {
  settings = {
    Lua = {
      diagnostics = { globals = { "vim" } },
      runtime = { version = "LuaJIT" },
      workspace = {
        checkThirdParty = false,
        library = { vim.env.VIMRUNTIME },
      },
      telemetry = { enable = false },
      hint = { enable = true },
    },
  },
}
