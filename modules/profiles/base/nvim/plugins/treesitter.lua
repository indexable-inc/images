-- Parsers are managed by Nix via nvim-treesitter.withAllGrammars,
-- so no ensure_installed list here; the grammars are already on disk.
require('nvim-treesitter.configs').setup {
  highlight = { enable = true },
  indent = { enable = true },
}
