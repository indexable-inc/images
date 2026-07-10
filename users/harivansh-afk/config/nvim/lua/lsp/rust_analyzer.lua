return {
  settings = {
    ["rust-analyzer"] = {
      checkOnSave = true,
      check = { command = "clippy" },
      cargo = { allFeatures = true },
      procMacro = { enable = true },
      diagnostics = { enable = true },
      inlayHints = {
        bindingModeHints = { enable = true },
        chainingHints = { enable = true },
        closingBraceHints = { enable = true },
        closureReturnTypeHints = { enable = "always" },
        lifetimeElisionHints = { enable = "always" },
        parameterHints = { enable = true },
        typeHints = { enable = true },
      },
    },
  },
}
