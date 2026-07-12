-- Local plugin: dispatch lines/selections to a headless Claude Code agent.
-- Implementation lives in lua/agent/init.lua.
return {
  dir = vim.fn.stdpath("config"),
  name = "claude-agent",
  event = "VeryLazy",
  config = function()
    require("agent").setup({
      -- mode = "bg",            -- "bg" = visible in `claude agents` (default)
      --                            "print" = detached one-shot, precise DONE/FAIL
      -- model = "opus",         -- pin a model if you want
    })
  end,
}
