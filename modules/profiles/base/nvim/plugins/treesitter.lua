-- Treesitter highlighting and indenting, driven by Neovim's own API.
--
-- nvim-treesitter 0.10 (the `main` rewrite, in nixpkgs since 2026-07) deleted
-- `nvim-treesitter.configs`. The plugin no longer turns features on; it is an
-- installer for parsers and nothing else. This image does not use that
-- installer -- `nvim-treesitter.withAllGrammars` puts every parser and its
-- queries on the runtimepath from the Nix store -- so there is no `setup{}` to
-- call here, no `ensure_installed`, and `:TSInstall` has nothing to do in a
-- read-only image.
--
-- What upstream now leaves to the editor is turning the highlighter on per
-- buffer. `vim.treesitter.start` asserts on a language with no parser, so
-- `language.add` is the guard: it returns nil rather than throwing when the
-- parser is not on the runtimepath, which is how a filetype we ship no grammar
-- for quietly stays on regex syntax instead of erroring on every open.
vim.api.nvim_create_autocmd("FileType", {
  group = vim.api.nvim_create_augroup("ix.treesitter", { clear = true }),
  desc = "Attach the treesitter highlighter and indenter where a parser exists",
  callback = function(ev)
    local lang = vim.treesitter.language.get_lang(ev.match)
    if not (lang and vim.treesitter.language.add(lang)) then
      return
    end
    vim.treesitter.start(ev.buf, lang)
    -- Indenting is the one feature nvim-treesitter still implements itself;
    -- Neovim has no built-in treesitter indenter.
    vim.bo[ev.buf].indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
  end,
})
