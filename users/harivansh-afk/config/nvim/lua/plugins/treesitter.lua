local function register_query_directive_compat()
  local query = require "vim.treesitter.query"

  local function capture_node(match, capture_id)
    local capture = match[capture_id]
    if type(capture) == "table" then return capture[1] end
    return capture
  end

  local html_script_type_languages = {
    ["importmap"] = "json",
    ["module"] = "javascript",
    ["application/ecmascript"] = "javascript",
    ["text/ecmascript"] = "javascript",
  }

  local non_filetype_match_injection_language_aliases = {
    ex = "elixir",
    pl = "perl",
    sh = "bash",
    uxn = "uxntal",
    ts = "typescript",
  }

  local function get_parser_from_markdown_info_string(injection_alias)
    local match = vim.filetype.match { filename = "a." .. injection_alias }
    return match or non_filetype_match_injection_language_aliases[injection_alias] or injection_alias
  end

  query.add_directive("set-lang-from-mimetype!", function(match, _, bufnr, pred, metadata)
    local node = capture_node(match, pred[2])
    if not node then return end

    local type_attr_value = vim.treesitter.get_node_text(node, bufnr)
    local configured = html_script_type_languages[type_attr_value]
    if configured then
      metadata["injection.language"] = configured
      return
    end

    local parts = vim.split(type_attr_value, "/", {})
    metadata["injection.language"] = parts[#parts]
  end, { force = true })

  query.add_directive("set-lang-from-info-string!", function(match, _, bufnr, pred, metadata)
    local node = capture_node(match, pred[2])
    if not node then return end

    local injection_alias = vim.treesitter.get_node_text(node, bufnr):lower()
    metadata["injection.language"] = get_parser_from_markdown_info_string(injection_alias)
  end, { force = true })

  query.add_directive("downcase!", function(match, _, bufnr, pred, metadata)
    local capture_id = pred[2]
    local node = capture_node(match, capture_id)
    if not node then return end

    local node_metadata = metadata[capture_id]
    local text = vim.treesitter.get_node_text(node, bufnr, { metadata = node_metadata }) or ""
    metadata[capture_id] = vim.tbl_extend("force", node_metadata or {}, { text = text:lower() })
  end, { force = true })
end

vim.pack.add({
  "https://github.com/nvim-treesitter/nvim-treesitter",
  "https://github.com/nvim-treesitter/nvim-treesitter-textobjects",
}, { load = function() end })

vim.api.nvim_create_autocmd("PackChanged", {
  callback = function(ev)
    local name, kind = ev.data.spec.name, ev.data.kind
    if kind == "delete" then return end
    if name == "nvim-treesitter" then vim.schedule(function() vim.cmd "TSUpdate all" end) end
  end,
})

return {
  {
    "nvim-treesitter/nvim-treesitter",
    after = function()
      require("nvim-treesitter").setup { auto_install = true }
      pcall(
        function() require("nvim-treesitter").install { "elixir", "heex", "eex", "markdown", "markdown_inline" } end
      )
      register_query_directive_compat()

      vim.api.nvim_create_autocmd("FileType", {
        callback = function(ev)
          if pcall(vim.treesitter.start, ev.buf) then
            vim.bo[ev.buf].indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
          end
        end,
      })
    end,
  },
  {
    "nvim-treesitter/nvim-treesitter-textobjects",
    after = function()
      require("nvim-treesitter-textobjects").setup {
        select = {
          enable = true,
          lookahead = true,
          keymaps = {
            ["af"] = "@function.outer",
            ["if"] = "@function.inner",
            ["ac"] = "@class.outer",
            ["ic"] = "@class.inner",
            ["aa"] = "@parameter.outer",
            ["ia"] = "@parameter.inner",
            ["ai"] = "@conditional.outer",
            ["ii"] = "@conditional.inner",
            ["al"] = "@loop.outer",
            ["il"] = "@loop.inner",
            ["ab"] = "@block.outer",
            ["ib"] = "@block.inner",
          },
        },
        move = {
          enable = true,
          set_jumps = true,
          goto_next_start = {
            ["]f"] = "@function.outer",
            ["]c"] = "@class.outer",
            ["]a"] = "@parameter.inner",
          },
          goto_next_end = {
            ["]F"] = "@function.outer",
            ["]C"] = "@class.outer",
          },
          goto_previous_start = {
            ["[f"] = "@function.outer",
            ["[c"] = "@class.outer",
            ["[a"] = "@parameter.inner",
          },
          goto_previous_end = {
            ["[F"] = "@function.outer",
            ["[C"] = "@class.outer",
          },
        },
        swap = {
          enable = true,
          swap_next = { ["<leader>sn"] = "@parameter.inner" },
          swap_previous = { ["<leader>sp"] = "@parameter.inner" },
        },
      }
    end,
  },
}
