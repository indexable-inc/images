-- Dispatch a line/selection to a Claude Code background agent.
--
-- Press the keymap on a line (normal mode) or over a selection (any visual /
-- select mode). The text is tagged `TODO`, sent to a real `claude --bg`
-- background session (so it shows up in `claude agents`), and the tag flips to
-- `DONE` when that session finishes. A spinner marks in-flight tasks.
--
-- Pure Lua: shells out to the `claude` binary on PATH (the dev images bake it
-- in), so there is no nixpkgs vimPlugin dependency. Wrapped in a `do ... end`
-- block because programs.neovim.configure concatenates every plugins/<n>.lua
-- into one chunk; the block keeps these locals out of that shared scope.
--
-- Launch modes (config.mode):
--   "bg"    -- `claude --bg`: visible in `claude agents`, attach/logs/stop.
--            Completion is detected by polling `claude agents --json` for the
--            session leaving "busy"; there is no exit code, so no FAIL state.
--   "print" -- `claude -p`: detached one-shot, NOT in agent view, but gives a
--            precise exit code (DONE / FAIL) and captured output.

do
  local ns = vim.api.nvim_create_namespace("claude_agent")

  local STATUS = { todo = "TODO", done = "DONE", fail = "FAIL" }
  local STATUS_RE = "^%s*[TODOFAILDONE]+%s+"

  local SPINNER = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" }

  -- Per-task records, keyed by extmark id, so a task survives the line moving.
  local tasks = {}
  -- Output of the most recently completed task, for show_output().
  local last_output = nil
  -- Resumable sessions, keyed by extmark id (persists after a task finishes so
  -- the DONE line stays resumable). Each entry: { id = short, full = sessionId }.
  local resumable = {}
  local last_resumable = nil

  local config = {
    cmd = "claude",
    mode = "bg", -- "bg" | "print"
    permission_mode = "bypassPermissions",
    model = nil, -- e.g. "opus"; nil = whatever claude defaults to
    poll_ms = 2500, -- bg: how often to poll `claude agents --json`
    poll_timeout_ms = 30 * 60 * 1000, -- bg: give up flipping to DONE after this
    extra_args = {},
  }

  local function strip_status(line)
    return (line:gsub(STATUS_RE, ""))
  end

  local function task_dir(buf)
    local name = vim.api.nvim_buf_get_name(buf)
    if name ~= "" then
      local dir = vim.fn.fnamemodify(name, ":p:h")
      if vim.fn.isdirectory(dir) == 1 then
        return dir
      end
    end
    return vim.fn.getcwd()
  end

  -- Return {start_line, end_line} (1-indexed, inclusive). Covers normal mode
  -- (current line) and v / V / <C-v> / select modes, then leaves visual mode.
  local function selection_range()
    local mode = vim.fn.mode()
    if mode:match("[vV\22sS\19]") then
      local s = vim.fn.getpos("v")[2]
      local e = vim.fn.getpos(".")[2]
      vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<Esc>", true, false, true), "nx", false)
      if s > e then
        s, e = e, s
      end
      return s, e
    end
    local cur = vim.api.nvim_win_get_cursor(0)[1]
    return cur, cur
  end

  -- Rewrite the line at the task's extmark to carry `status` + a virt_text marker.
  local function set_status(rec, status, virt)
    if not vim.api.nvim_buf_is_valid(rec.buf) then
      return
    end
    local pos = vim.api.nvim_buf_get_extmark_by_id(rec.buf, ns, rec.mark, {})
    if not pos[1] then
      return
    end
    local row = pos[1]
    local line = vim.api.nvim_buf_get_lines(rec.buf, row, row + 1, false)[1] or ""
    local indent = line:match("^%s*") or ""
    local bare = strip_status(line):gsub("^%s*", "")
    local new = indent .. STATUS[status] .. " " .. bare
    if new ~= line then
      vim.api.nvim_buf_set_lines(rec.buf, row, row + 1, false, { new })
    end
    local hl = ({ todo = "DiagnosticInfo", done = "DiagnosticOk", fail = "DiagnosticError" })[status]
    rec.mark = vim.api.nvim_buf_set_extmark(rec.buf, ns, row, 0, {
      id = rec.mark,
      virt_text = virt and { { virt, hl } } or nil,
      virt_text_pos = "eol",
    })
  end

  local function stop_timer(rec, key)
    if rec[key] then
      rec[key]:stop()
      rec[key]:close()
      rec[key] = nil
    end
  end

  local function start_spinner(rec)
    local i = 0
    rec.spinner = vim.uv.new_timer()
    rec.spinner:start(0, 100, vim.schedule_wrap(function()
      if not vim.api.nvim_buf_is_valid(rec.buf) then
        stop_timer(rec, "spinner")
        return
      end
      i = (i % #SPINNER) + 1
      local hint = rec.id and (SPINNER[i] .. " " .. rec.id) or (SPINNER[i] .. " starting")
      set_status(rec, "todo", hint)
    end))
  end

  local function build_prompt(task)
    return "Complete this task. Be autonomous and finish it end to end.\n\nTask:\n" .. task
  end

  local function base_args()
    local a = { "--permission-mode", config.permission_mode }
    if config.model then
      vim.list_extend(a, { "--model", config.model })
    end
    vim.list_extend(a, config.extra_args)
    return a
  end

  local function finish(rec, status, virt)
    stop_timer(rec, "spinner")
    stop_timer(rec, "poll")
    set_status(rec, status, virt)
    tasks[rec.mark] = nil
  end

  -- "bg" mode: poll `claude agents --json` until the session leaves "busy".
  local function start_poll(rec)
    rec.polls = 0
    rec.idle = 0
    rec.elapsed = 0
    rec.poll = vim.uv.new_timer()
    rec.poll:start(config.poll_ms, config.poll_ms, vim.schedule_wrap(function()
      rec.elapsed = rec.elapsed + config.poll_ms
      if rec.elapsed >= config.poll_timeout_ms then
        finish(rec, "todo", "⏱ poll timed out · " .. (rec.id or "?"))
        return
      end
      vim.system({ config.cmd, "agents", "--json" }, { text = true }, function(obj)
        vim.schedule(function()
          if not tasks[rec.mark] then
            return
          end
          local ok, list = pcall(vim.json.decode, obj.stdout or "")
          if not ok or type(list) ~= "table" then
            return
          end
          local found
          for _, s in ipairs(list) do
            if type(s.sessionId) == "string" and s.sessionId:sub(1, 8) == rec.id then
              found = s
              break
            end
          end
          if found then
            rec.full = found.sessionId
          end
          rec.polls = rec.polls + 1
          local done
          if found then
            if found.status == "busy" then
              rec.seen_busy = true
              rec.idle = 0
            else
              rec.idle = rec.idle + 1
            end
            -- busy -> not-busy = finished; or never-busy but idle a while = quick finish
            done = (rec.seen_busy and found.status ~= "busy") or (not rec.seen_busy and rec.idle >= 2)
          else
            -- gone from the list: finished and cleaned, or never appeared
            done = rec.seen_busy or rec.polls >= 3
          end
          if done then
            local entry = { id = rec.id, full = rec.full or rec.id, dir = rec.dir }
            resumable[rec.mark] = entry
            last_resumable = entry
            finish(rec, "done", "✓ " .. (rec.id or "") .. " · <leader>ar resume")
            vim.notify("agent: done " .. (rec.id or "") .. " — <leader>ar to resume", vim.log.levels.INFO)
          end
        end)
      end)
    end))
  end

  local function dispatch()
    local buf = vim.api.nvim_get_current_buf()
    local s, e = selection_range()

    local lines = vim.api.nvim_buf_get_lines(buf, s - 1, e, false)
    local task = strip_status(table.concat(lines, "\n")):gsub("^%s+", "")
    if task == "" then
      vim.notify("agent: empty task", vim.log.levels.WARN)
      return
    end

    local dir = task_dir(buf)
    local mark = vim.api.nvim_buf_set_extmark(buf, ns, s - 1, 0, {})
    local rec = { buf = buf, mark = mark, task = task, dir = dir }
    tasks[mark] = rec
    set_status(rec, "todo", "⠋ starting")
    start_spinner(rec)

    local prompt = build_prompt(task)

    if config.mode == "bg" then
      local name = task:gsub("%s+", " "):sub(1, 50)
      local args = vim.list_extend({ config.cmd, "--bg", "-n", name }, base_args())
      table.insert(args, prompt)
      vim.system(args, { cwd = dir, text = true }, function(obj)
        vim.schedule(function()
          if not tasks[mark] then
            return
          end
          local id = (obj.stdout or ""):match("(%x%x%x%x%x%x%x%x)")
          if obj.code ~= 0 or not id then
            finish(rec, "fail", "✗ launch failed (" .. tostring(obj.code) .. ")")
            last_output = (obj.stdout or "") .. "\n" .. (obj.stderr or "")
            vim.notify("agent: launch failed", vim.log.levels.ERROR)
            return
          end
          rec.id = id
          vim.notify("agent: backgrounded " .. id .. " (" .. vim.fn.fnamemodify(dir, ":~") .. ")", vim.log.levels.INFO)
          start_poll(rec)
        end)
      end)
    else -- "print"
      local args = vim.list_extend({ config.cmd, "-p", prompt }, base_args())
      vim.notify("agent: dispatched in " .. vim.fn.fnamemodify(dir, ":~"), vim.log.levels.INFO)
      vim.system(args, { cwd = dir, text = true }, function(obj)
        vim.schedule(function()
          if not tasks[mark] then
            return
          end
          if obj.code == 0 then
            last_output = obj.stdout or ""
            finish(rec, "done", "✓ done")
            vim.notify("agent: done — :ClaudeAgentOutput to view", vim.log.levels.INFO)
          else
            last_output = (obj.stdout or "") .. "\n" .. (obj.stderr or "")
            finish(rec, "fail", "✗ failed (" .. tostring(obj.code) .. ")")
            vim.notify("agent: failed (" .. tostring(obj.code) .. ")", vim.log.levels.ERROR)
          end
        end)
      end)
    end
  end

  -- Open `cmd_list` as an interactive terminal (PTY) in a split, in `dir`.
  local function open_term(dir, cmd_list)
    vim.cmd("botright new")
    vim.fn.jobstart(cmd_list, { term = true, cwd = dir })
    vim.cmd("startinsert")
  end

  -- Open Claude's agent view in a terminal split.
  local function agent_view()
    vim.cmd("botright split")
    vim.cmd("terminal " .. config.cmd .. " agents")
    vim.cmd("startinsert")
  end

  -- Show output: agent view (bg) or the captured -p output (print).
  -- NOTE: this claude only implements `claude agents`; `logs`/`attach`/`stop`
  -- are advertised by `--bg` but unimplemented (they spawn a stray session if
  -- invoked), so bg mode routes to the agent view to read/attach there.
  local function show_output()
    if config.mode == "bg" then
      agent_view()
      return
    end
    if not last_output or last_output == "" then
      vim.notify("agent: no output yet", vim.log.levels.WARN)
      return
    end
    vim.cmd("botright new")
    local b = vim.api.nvim_get_current_buf()
    vim.bo[b].buftype = "nofile"
    vim.bo[b].bufhidden = "wipe"
    vim.bo[b].filetype = "markdown"
    vim.api.nvim_buf_set_lines(b, 0, -1, false, vim.split(last_output, "\n"))
  end

  -- Resume the agent whose task lives on the current line (or the most recent).
  --
  -- `claude --resume <id>` is cwd-scoped (run it in the task's dir) and is
  -- REJECTED while the session is still alive as a bg agent ("running as bg").
  -- A bg agent lingers ~1h after finishing its task, so to continue it we first
  -- stop its (idle) process, then resume from the jobs transcript. If it is
  -- still busy we do not kill it; we open the agent view so you can attach.
  local function resume()
    local buf = vim.api.nvim_get_current_buf()
    local row = vim.api.nvim_win_get_cursor(0)[1] - 1
    local marks = vim.api.nvim_buf_get_extmarks(buf, ns, { row, 0 }, { row, -1 }, {})
    local target
    for _, m in ipairs(marks) do
      local id = m[1]
      if resumable[id] then
        target = resumable[id]
        break
      elseif tasks[id] and tasks[id].full then
        target = { id = tasks[id].id, full = tasks[id].full, dir = tasks[id].dir }
        break
      end
    end
    target = target or last_resumable
    if not target then
      vim.notify("agent: no resumable session on this line", vim.log.levels.WARN)
      return
    end

    local function do_resume()
      open_term(target.dir, { config.cmd, "--resume", target.full or target.id })
    end

    -- Look up the live session: stop it if idle so --resume is allowed.
    vim.system({ config.cmd, "agents", "--json" }, { text = true }, function(obj)
      vim.schedule(function()
        local ok, list = pcall(vim.json.decode, obj.stdout or "")
        local entry
        if ok and type(list) == "table" then
          for _, sn in ipairs(list) do
            if sn.sessionId == (target.full or target.id) then
              entry = sn
              break
            end
          end
        end
        if entry and entry.status == "busy" then
          vim.notify("agent: still running — opening agent view to attach", vim.log.levels.WARN)
          agent_view()
          return
        end
        if entry and entry.pid then
          pcall(vim.uv.kill, entry.pid, "sigterm")
          -- give the process a moment to release the session before resuming
          vim.defer_fn(do_resume, 800)
        else
          -- already stopped/reaped: resume directly
          do_resume()
        end
      end)
    end)
  end

  vim.keymap.set("n", "<leader>aa", dispatch, { desc = "Agent: dispatch current line" })
  vim.keymap.set("x", "<leader>aa", dispatch, { desc = "Agent: dispatch selection" })
  vim.keymap.set("n", "<leader>ao", show_output, { desc = "Agent: show last output / logs" })
  vim.keymap.set("n", "<leader>av", agent_view, { desc = "Agent: open `claude agents` view" })
  vim.keymap.set("n", "<leader>ar", resume, { desc = "Agent: resume the session on this line" })

  vim.api.nvim_create_user_command("ClaudeAgent", dispatch, { desc = "Dispatch current line to Claude agent" })
  vim.api.nvim_create_user_command("ClaudeAgentOutput", show_output, { desc = "Show last Claude agent output/logs" })
  vim.api.nvim_create_user_command("ClaudeAgentView", agent_view, { desc = "Open claude agents view" })
  vim.api.nvim_create_user_command("ClaudeAgentResume", resume, { desc = "Resume the agent on the current line" })
end
