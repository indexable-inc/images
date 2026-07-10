vim.pack.add({
  "https://github.com/ibhagwan/fzf-lua",
}, { load = function() end })

---@param kind 'issue'|'pr'
---@param state 'all'|'open'|'closed'
local function gh_picker(kind, state)
  if vim.fn.executable "gh" ~= 1 then
    vim.notify("gh CLI not found", vim.log.levels.WARN)
    return
  end

  local next_state = ({ all = "open", open = "closed", closed = "all" })[state]
  local label = kind == "pr" and "PRs" or "Issues"

  require("fzf-lua").fzf_exec(("gh %s list --limit 100 --state %s"):format(kind, state), {
    prompt = ("%s (%s)> "):format(label, state),
    header = ":: <c-o> to toggle all/open/closed",
    actions = {
      ["default"] = function(selected)
        local num = selected[1]:match "^#?(%d+)"
        if num then vim.system { "gh", kind, "view", num, "--web" } end
      end,
      ["ctrl-o"] = function() gh_picker(kind, next_state) end,
    },
  })
end

return {
  "ibhagwan/fzf-lua",
  cmd = "FzfLua",
  before = function()
    pcall(vim.cmd.packadd, "nvim-web-devicons")
    pcall(vim.cmd.packadd, "nonicons.nvim")
  end,
  after = function()
    local fzf = require "fzf-lua"
    local opts = {
      "default-title",
      winopts = {
        fullscreen = true,
        border = "single",
        preview = {
          layout = "vertical",
          vertical = "down:50%",
        },
      },
      fzf_opts = {
        ["--layout"] = "reverse",
      },
      files = {
        winopts = { preview = { hidden = true } },
      },
      git_files = {
        winopts = { preview = { hidden = true } },
      },
      grep = {
        _headers = {},
      },
      live_grep = {
        _headers = {},
      },
    }

    fzf.setup(opts)

    local ok, fzf_reload = pcall(require, "config.fzf_reload")
    if ok then
      fzf_reload.setup(opts)
      fzf_reload.reload()
    end
  end,
  keys = {
    { "<C-f>", "<cmd>FzfLua files<cr>" },
    { "<C-s>", "<cmd>FzfLua live_grep<cr>" },
    { "<leader>ff", "<cmd>FzfLua files<cr>" },
    { "<leader>fg", "<cmd>FzfLua live_grep<cr>" },
    { "<leader>fb", "<cmd>FzfLua buffers<cr>" },
    { "<leader>fh", "<cmd>FzfLua help_tags<cr>" },
    { "<leader>fr", "<cmd>FzfLua resume<cr>" },
    { "<leader>fo", "<cmd>FzfLua oldfiles<cr>" },
    { "<leader>fc", "<cmd>FzfLua commands<cr>" },
    { "<leader>fk", "<cmd>FzfLua keymaps<cr>" },
    { "<leader>f/", "<cmd>FzfLua search_history<cr>" },
    { "<leader>f:", "<cmd>FzfLua command_history<cr>" },
    { "<leader>fe", "<cmd>FzfLua files cwd=~/.config<cr>" },
    { "gq", "<cmd>FzfLua quickfix<cr>" },
    { "gl", "<cmd>FzfLua loclist<cr>" },
    { "<leader>GB", "<cmd>FzfLua git_branches<cr>" },
    { "<leader>Gc", "<cmd>FzfLua git_commits<cr>" },
    { "<leader>Gs", "<cmd>FzfLua git_status<cr>" },
    {
      "<leader>Gp",
      function() gh_picker("pr", "open") end,
    },
    {
      "<leader>Gi",
      function() gh_picker("issue", "open") end,
    },
  },
}
