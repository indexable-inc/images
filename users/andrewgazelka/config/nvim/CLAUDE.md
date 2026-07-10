# CLAUDE.md - Neovim Configuration

This file provides guidance to Claude Code when working with the Neovim configuration in this directory.

## Architecture Overview

This is a **modern Neovim configuration** using:

- **lazy.nvim** for plugin management
- **Native vim.lsp.config API** (2025 standard) for LSP configuration
- **Mason** for LSP server installation
- **Modular structure** with separate config and plugin files

### Directory Structure

```
nvim/
├── init.lua                 # Entry point - bootstraps lazy.nvim and loads modules
├── lua/
│   ├── config/             # Core Neovim configuration
│   │   ├── options.lua     # Vim options and settings
│   │   ├── keymaps.lua     # Global keymaps
│   │   └── autocmds.lua    # Autocommands and event handlers
│   └── plugins/            # Plugin configurations (lazy.nvim auto-loads these)
│       ├── lsp.lua         # LSP, Mason, and autocompletion (blink.cmp)
│       ├── languages.lua   # Treesitter and language-specific plugins
│       ├── ui.lua          # UI enhancements (folding, sticky scroll)
│       ├── colorscheme.lua # Colorscheme configuration
│       ├── editing.lua     # Text editing plugins (surround, autopairs, etc.)
│       ├── motion.lua      # Navigation and motion plugins
│       ├── snacks.lua      # Snacks.nvim (picker, dashboard, etc.)
│       ├── git.lua         # Git integration
│       └── file-explorer.lua # File explorer
```

## Key Components

### Plugin Management (init.lua:1-28)

- **Bootstrap process**: Auto-installs lazy.nvim if not present
- **Loading order**: options → autocmds → plugins → keymaps
- Plugins are auto-loaded from `lua/plugins/` directory
- Each file in `plugins/` should return a table (or array of tables) of plugin specs

### Core Configuration (lua/config/)

**options.lua** - Vim settings including:
- Leader key: `<Space>` (MUST be set before plugins load)
- Persistent undo/history with shada configuration
- No swap files (uses git + auto-save instead)
- Transparent background approach (disabled backgrounds)
- Folding settings optimized for nvim-ufo

**keymaps.lua** - Global keybindings:
- Snacks.nvim picker keymaps (`<leader>ff`, `<leader>fg`, etc.)
- Spell check keymaps (`<leader>st`, `<leader>sn`, etc.)
- Plugin-specific keymaps are typically defined in their plugin configs

**autocmds.lua** - Event handlers for:
- Auto-create directories on save
- Restore cursor position
- File-type specific settings (markdown, gitcommit, etc.)
- **Transparent background enforcement**: ColorScheme autocmd applies `bg = 'none'` to all highlight groups
- Markdown-specific keybinds (Cmd+i for italic, Cmd+b for bold)

### LSP Configuration (lua/plugins/lsp.lua)

**IMPORTANT PATTERNS:**

1. **Modern LSP API (2025)**:
   - Uses `vim.lsp.config.*` for server configuration
   - Uses `vim.lsp.enable()` to start servers
   - NO nvim-lspconfig's `require('lspconfig').server.setup()` pattern

2. **Mason Integration**:
   - Mason installs LSP servers
   - mason-lspconfig bridges Mason and LSP
   - `ensure_installed` list in mason-lspconfig

3. **LSP Servers Configuration**:
   - Each server uses: `cmd`, `filetypes`, `root_markers`, `capabilities`, `on_attach`, `settings`
   - Semantic tokens DISABLED (too slow, use treesitter)
   - Folding support enabled for nvim-ufo

4. **Completion**:
   - Uses **blink.cmp** (2025 standard, not nvim-cmp)
   - Disabled in markdown files
   - Tab accepts completion and jumps forward in snippets

5. **Performance Optimizations**:
   - LSP logging disabled
   - Semantic tokens disabled
   - Force LSP shutdown on VimLeavePre (prevents :wq delays)

6. **Ghostty Integration**:
   - LSP progress displayed in Ghostty's native progress bar
   - Uses OSC 9;4 escape sequences

### Language Support (lua/plugins/languages.lua)

**Treesitter**:
- Auto-install missing parsers
- Essential parsers in `ensure_installed` (git, markdown, etc.)
- Additional vim regex highlighting enabled for markdown code blocks

**Language-Specific Plugins**:
- Rust: rust-tools.nvim
- Cargo.toml: crates.nvim with LSP integration
- Typst: typst.vim with concealment
- Nushell: Currently commented out

### UI Plugins (lua/plugins/ui.lua)

**nvim-ufo** - Modern folding:
- LSP-based folding with treesitter fallback
- Markdown uses LSP → treesitter → indent priority
- Keymaps: `zR`, `zM`, `zr`, `zm`, `zp`

**nvim-treesitter-context** - Sticky scroll:
- Shows context at top (like VSCode)
- Max 3 lines, minimum 20-line window

**fidget.nvim** - Progress notifications:
- Bottom-right minimal notifications
- No window blending

### Text Editing (lua/plugins/editing.lua)

**Key Features**:
- vim-surround for text objects
- bullets.vim for markdown lists
- nvim-autopairs with IntelliJ-style Tab behavior
- multiple-cursors.nvim for VS Code-like multiple cursors (Alt+n)

**Custom Tab Behavior**:
- Jumps over closing brackets: `)`, `]`, `}`, `"`, `'`, `` ` ``, `>`
- Jumps over markdown formatting: `**` (bold), `*` (italic)
- Falls back to normal Tab otherwise

### Multiple Cursors

**Current Plugin**: `brenton-leighton/multiple-cursors.nvim`

**Keybindings**:
- `Alt+n` - add cursor at next match (incremental, like VS Code Ctrl+d)
- `Alt+N` - add cursors to ALL matches at once
- `Alt+x` - skip current match, jump to next
- `↑/↓` - add cursor above/below
- `Ctrl+↑/↓` - add cursor above/below (works in insert mode too)
- `Ctrl+click` - add/remove cursor with mouse
- `Esc` - exit multicursor mode

**Why This Plugin**:
We evaluated several multicursor plugins. The key differentiator is **live updates** - seeing edits happen immediately at all cursor positions vs batched/delayed updates.

#### Multicursor Plugin Comparison

| Plugin | Normal Mode Edits | Insert Mode Edits | Notes |
|--------|-------------------|-------------------|-------|
| **brenton-leighton/multiple-cursors.nvim** | Live | Live | **Current choice**. Edits show immediately at all cursors. Works "like normal Neovim". |
| jake-stewart/multicursor.nvim | Batched (shows on Esc) | Live | Uses SafeState autocmd. Non-main cursors don't update until you press Esc. Most popular (~1.1k stars). |
| smoka7/multicursors.nvim | Only mapped keys | Live (50ms updatetime) | Uses hydra.nvim. Normal mode requires explicit key mappings. |
| mg979/vim-visual-multi | Live | Live | Vimscript-based (not Lua). Was the previous choice. Works well but older codebase. |

#### Tradeoffs

**brenton-leighton/multiple-cursors.nvim** (current):
- Pros: True live updates, works with almost all normal Neovim commands, split-paste feature
- Cons: Fewer stars/less popular, requires adding custom keymaps to `custom_key_maps` table

**jake-stewart/multicursor.nvim**:
- Pros: Most popular, extensive Cursor API for custom logic, good documentation
- Cons: Normal mode edits are batched - you don't see changes at other cursors until Esc

**smoka7/multicursors.nvim**:
- Pros: Built on hydra.nvim, extend mode for expanding selections
- Cons: Normal mode only works with explicitly mapped keys

**mg979/vim-visual-multi**:
- Pros: Battle-tested, lots of features, live updates
- Cons: Vimscript (not Lua), complex codebase, can be slow on large files

### Colorscheme (lua/plugins/colorscheme.lua)

**Active Theme**: JetBrains (jb.nvim)

**Transparent Background**:
- Explicitly sets `bg = 'none'` for ALL highlight groups
- This is ALSO enforced by ColorScheme autocmd in autocmds.lua
- Ensures transparency persists across colorscheme changes

## Development Workflow

### Adding a New Plugin

1. **Create or edit a file in `lua/plugins/`**:
   ```lua
   return {
     "author/plugin-name",
     config = function()
       -- Plugin configuration here
     end,
   }
   ```

2. **Restart Neovim** - lazy.nvim auto-loads the file

3. **No need to run home-manager switch** - nvim config is symlinked

### Adding LSP Server

1. **Add to mason-lspconfig's `ensure_installed`** (lsp.lua:21):
   ```lua
   ensure_installed = { "lua_ls", "pyright", "taplo", "marksman", "new_server" }
   ```

2. **Configure the server using vim.lsp.config** (lsp.lua:~88+):
   ```lua
   vim.lsp.config.new_server = {
     cmd = { "new-server" },
     filetypes = { "filetype" },
     root_markers = { "marker.toml" },
     capabilities = capabilities,
     on_attach = on_attach,
     settings = {
       -- Server-specific settings
     },
   }
   ```

3. **Enable the server** (lsp.lua:178):
   ```lua
   vim.lsp.enable({ "lua_ls", "pyright", "taplo", "marksman", "new_server" })
   ```

4. **Restart Neovim** - Mason will auto-install if available

### Adding Treesitter Parser

1. **Add to `ensure_installed`** in languages.lua:11:
   ```lua
   ensure_installed = {
     "git_config", "gitcommit", "markdown",
     "new_language",  -- Add here
   }
   ```

2. **Restart Neovim** - Auto-installs on next startup

### Modifying Keymaps

**Global keymaps**: Edit `lua/config/keymaps.lua`
**Plugin-specific keymaps**: Edit in the plugin's config (usually in `keys` or `config` function)
**LSP keymaps**: Edit `on_attach` function in `lua/plugins/lsp.lua:57-85`

### File-Type Specific Settings

Add autocommands in `lua/config/autocmds.lua` using the FileType pattern:

```lua
vim.api.nvim_create_autocmd({"FileType"}, {
  pattern = {"your_filetype"},
  callback = function()
    -- Settings here
  end,
})
```

## Common Patterns

### Plugin Lazy Loading

```lua
return {
  "author/plugin",
  ft = { "filetype" },  -- Load on filetype
  event = "VeryLazy",   -- Load after startup
  keys = {              -- Load on keymap
    { "<leader>x", "<cmd>Command<cr>", desc = "Description" },
  },
  cmd = "Command",      -- Load on command
}
```

### Transparent Background

**Two enforcement points**:
1. Colorscheme plugin config (colorscheme.lua:12-76)
2. ColorScheme autocmd (autocmds.lua:86-158)

This ensures transparency works even when switching colorschemes at runtime.

### Markdown Handling

**Special behaviors**:
- Spell check auto-enabled
- Soft wrap on word boundaries
- List continuation (bullets.vim)
- Cmd+i/Cmd+b for italic/bold formatting
- Inlay hints disabled
- Additional vim regex highlighting for code blocks

### Performance Optimizations

1. **LSP**: Semantic tokens disabled, logging off, force shutdown
2. **Swap files**: Disabled (uses git + persistent undo)
3. **Treesitter**: Selective parser installation
4. **Lazy loading**: Plugins load on-demand via ft, event, keys, cmd

## Important Notes

### Do NOT Do These Things

1. **Don't use old lspconfig API**: Use `vim.lsp.config.*` not `require('lspconfig').server.setup()`
2. **Don't install LSP servers via Nix**: Use Mason (via lazy.nvim)
3. **Don't create new highlight groups**: They'll be overridden by transparent background autocmd
4. **Don't disable the ColorScheme autocmd**: It's essential for maintaining transparency
5. **Don't use nvim-cmp**: This config uses blink.cmp (2025 standard)

### When Things Go Wrong

**LSP not working**:
1. Check Mason installation: `:Mason`
2. Check LSP status: `:LspInfo`
3. Check server is in `ensure_installed` AND `vim.lsp.enable()`

**Plugin not loading**:
1. Check lazy.nvim: `:Lazy`
2. Verify file is in `lua/plugins/` and returns a table
3. Check for syntax errors: `:messages`

**Keymaps not working**:
1. Check load order (keymaps load AFTER plugins)
2. Plugin keymaps might override global ones
3. Use `:map <key>` to see what's mapped

## Git Workflow

**Commit patterns** (follow repository style):
- Lowercase imperative mood ("add", "update", "fix")
- Specific but concise
- Examples: "add rust-analyzer support", "fix transparent background", "update lsp keybindings"

**Atomic commits**:
- Group related changes (e.g., plugin + keymap + autocmd)
- Separate unrelated changes into different commits

## References

**Plugin Documentation**:
- lazy.nvim: https://github.com/folke/lazy.nvim
- blink.cmp: https://github.com/saghen/blink.cmp
- nvim-ufo: https://github.com/kevinhwang91/nvim-ufo

**Neovim APIs**:
- LSP: `:h vim.lsp.config`, `:h vim.lsp.enable()`
- Autocommands: `:h nvim_create_autocmd`
- Keymaps: `:h vim.keymap.set`
