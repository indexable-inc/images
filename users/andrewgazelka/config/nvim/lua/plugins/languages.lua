-- Language-specific plugins

return {
  -- Treesitter for syntax highlighting and parsing
  {
    "nvim-treesitter/nvim-treesitter",
    build = ":TSUpdate",
    config = function()
      -- New nvim-treesitter API (2025) - no more configs module
      require("nvim-treesitter").setup({
        -- Ensure these parsers are installed
        ensure_installed = {
          -- Git functionality
          "git_config", "gitcommit", "git_rebase", "gitignore", "gitattributes", "diff",
          -- Markdown with code block highlighting
          "markdown", "markdown_inline",
          -- Bash (also used for dotenv highlighting)
          "bash",
          -- Nushell
          "nu",
          -- Languages used in markdown code fences; pre-install so treesitter
          -- can inject them synchronously (auto_install fires too late to
          -- colour the first render of a block).
          "rust", "lua", "python", "javascript", "typescript", "tsx",
          "go", "c", "cpp", "json", "yaml", "toml", "html", "css", "sql",
        },

        -- Install parsers synchronously (only applied to `ensure_installed`)
        sync_install = false,

        -- Automatically install missing parsers when entering buffer
        auto_install = true,

        -- List of parsers to ignore installing (for "all")
        ignore_install = {},
      })

      -- Auto-start treesitter highlighting for any filetype with an installed parser
      vim.api.nvim_create_autocmd("FileType", {
        callback = function(args)
          local ft = vim.bo[args.buf].filetype
          if ft == "" then return end
          pcall(vim.treesitter.start, args.buf)
        end,
      })

      -- Enable treesitter highlighting
      vim.treesitter.language.register("markdown", "markdown")
      -- Use bash parser for dotenv files (provides syntax highlighting without LSP interference)
      vim.treesitter.language.register("bash", "dotenv")
    end,
  },

  -- Markdown rendering: Notion-style code blocks and inline-code badges.
  {
    "MeanderingProgrammer/render-markdown.nvim",
    dependencies = { "nvim-treesitter/nvim-treesitter" },
    ft = { "markdown" },
    opts = {
      anti_conceal = { enabled = false },
      render_modes = true,
      win_options = {
        conceallevel = { default = 0, rendered = 0 },
        concealcursor = { default = "", rendered = "" },
      },
      code = {
        sign = false,
        conceal_delimiters = false,  -- keep the ``` lines visible
        language = false,
        width = "block",
        min_width = 50,
        left_pad = 2,
        right_pad = 2,
        style = "normal",
        border = "hide",
        inline = true,
        inline_pad = 1,
      },
      heading  = { enabled = false },
      bullet   = { enabled = false },
      checkbox = { enabled = false },
      dash     = { enabled = false },
      link     = { enabled = false },
      quote    = { enabled = false },
      pipe_table = { enabled = false },
    },
    config = function(_, opts)
      require("render-markdown").setup(opts)

      local highlights = require("config.highlights")
      highlights.apply_markdown_notion()
      vim.api.nvim_create_autocmd("ColorScheme", { pattern = "*", callback = highlights.apply_markdown_notion })
      vim.api.nvim_create_autocmd("OptionSet", { pattern = "background", callback = highlights.apply_markdown_notion })
    end,
  },

  -- Typst support
  {
    "kaarmu/typst.vim",
    ft = { "typst", "typ" },
    config = function()
      -- Enable concealment for italic/bold
      vim.g.typst_conceal = 1
      -- Enable concealment for math symbols
      vim.g.typst_conceal_math = 1
      -- Enable emoji concealment
      vim.g.typst_conceal_emoji = 1
      -- Auto-open quickfix for errors
      vim.g.typst_auto_open_quickfix = 1
      -- Enable syntax highlighting for embedded languages
      vim.g.typst_embedded_languages = { 'python', 'javascript', 'typescript', 'lua', 'bash', 'c', 'cpp', 'go', 'java' }
      -- Enable folding for headings
      vim.g.typst_folding = 1
    end,
  },

}
