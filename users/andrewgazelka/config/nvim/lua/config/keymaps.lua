-- Keymaps configuration

-- Close without saving
vim.keymap.set('n', '<C-c>', '<cmd>q!<cr>', { desc = "Close without saving" })

-- Snacks.nvim picker keymaps
vim.keymap.set('n', '<leader>ff', function() require("snacks").picker.files() end, { desc = "Find Files" })
vim.keymap.set('n', '<leader>fg', function() require("snacks").picker.grep() end, { desc = "Grep Files" })
vim.keymap.set('n', '<leader>fb', function() require("snacks").picker.buffers() end, { desc = "Find Buffers" })
vim.keymap.set('n', '<leader>fr', function() require("snacks").picker.recent() end, { desc = "Recent Files" })
vim.keymap.set('n', '<leader>fw', function() require("snacks").picker.grep_word() end, { desc = "Grep Word" })
vim.keymap.set('n', '<leader>/', function() require("snacks").picker.grep() end, { desc = "Search in Files" })

-- Flash (motion) keymaps are handled by the plugin's keys config

-- Visual mode keymaps
vim.keymap.set('v', 'q', 'y:q<CR>', { desc = "Copy selection and quit" })

-- Cmd+A to select all (like standard macOS behavior)
vim.keymap.set('n', '<D-a>', 'ggVG', { desc = "Select all" })
vim.keymap.set('v', '<D-a>', 'ggVG', { desc = "Select all" })
vim.keymap.set('i', '<D-a>', '<Esc>ggVG', { desc = "Select all" })

-- Cmd+E to quit, matching :q
vim.keymap.set({ 'n', 'v' }, '<D-e>', '<cmd>q<cr>', { desc = "Quit" })
vim.keymap.set('i', '<D-e>', '<Esc><cmd>q<cr>', { desc = "Quit" })

-- Option+Backspace to delete previous word
vim.keymap.set('i', '<M-BS>', '<C-w>', { noremap = true, desc = "Delete previous word" })
vim.keymap.set('c', '<M-BS>', '<C-w>', { noremap = true, desc = "Delete previous word" })

-- Spell check keymaps
vim.keymap.set('n', '<leader>st', '<cmd>setlocal spell!<cr>', { desc = "Toggle spell check" })
vim.keymap.set('n', '<leader>sn', ']s', { desc = "Next misspelled word" })
vim.keymap.set('n', '<leader>sp', '[s', { desc = "Previous misspelled word" })
vim.keymap.set('n', '<leader>sa', 'zg', { desc = "Add word to dictionary" })
vim.keymap.set('n', '<leader>su', 'zug', { desc = "Undo add word to dictionary" })
vim.keymap.set('n', '<leader>sr', 'z=', { desc = "Show spelling suggestions" })
vim.keymap.set('n', '<leader>sw', 'zw', { desc = "Mark word as wrong" })
