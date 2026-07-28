-- File explorer as a buffer: `-` opens the parent dir as an editable
-- buffer where renames/deletes are made by editing text and saving.
require('oil').setup {}
vim.keymap.set('n', '-', '<CMD>Oil<CR>', { desc = 'Open parent directory' })
