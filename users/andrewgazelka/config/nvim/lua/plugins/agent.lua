-- Local plugin: dispatch lines/selections to a headless Claude Code agent.
-- The implementation is shared with index's base Neovim profile and installed
-- declaratively at lua/agent/shared.lua by the development profile.
return {
  dir = vim.fn.stdpath("config"),
  name = "claude-agent",
  event = "VeryLazy",
  config = function()
    dofile(vim.fn.stdpath("config") .. "/lua/agent/shared.lua")
  end,
}
