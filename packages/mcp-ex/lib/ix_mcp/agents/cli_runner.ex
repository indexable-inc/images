defmodule IxMcp.Agents.CliRunner do
  @moduledoc """
  The `AgentHarness.Runner` that executes one working phase of a child as
  a real agent CLI process, speaking each backend's line-JSON protocol:

    * claude/kimi: `-p --input/--output-format stream-json`. The brief
      goes in as a stream-json user line and stdin stays open: that is
      the message-injection channel. After every tool result the runner
      calls `AgentHarness.checkpoint/2` and each queued lead message
      becomes another user turn (the card's delivery rule).
    * codex: `exec --json` JSONL. No stdin channel exists, so no
      checkpoint mid-run: lead messages stay queued and wake the idle
      child through `exec resume <thread>`.

  Every parsed line is recorded as a normalized event in
  `IxMcp.Agents.Events` (which feeds the board graph); the final text
  returns to the harness, which routes it to the lead.
  """

  @behaviour AgentHarness.Runner

  alias AgentHarness.Context
  alias IxMcp.Agents.Backend
  alias IxMcp.Agents.Events

  # A child silent for this long is dead to us; the error reaches the
  # lead, which can respawn or investigate. Per spawn: :idle_timeout_ms.
  @idle_timeout_ms 15 * 60 * 1000

  @impl true
  def run(instructions, %Context{} = ctx) do
    backend = Keyword.get(ctx.opts, :backend, :claude)

    spec =
      Backend.command(
        backend,
        ctx.opts
        |> Keyword.put(:model, ctx.model)
        |> Keyword.put(:resume, Events.session_ref(ctx.agent_id))
        |> Keyword.put(:prompt, instructions)
      )

    cwd = validate_cwd!(ctx.opts)
    port = open_port(spec, cwd)
    if spec.stdin == :stream, do: inject_user(port, instructions)

    loop(%{
      ctx: ctx,
      backend: backend,
      port: port,
      cwd: cwd,
      stdin: spec.stdin,
      buf: "",
      final: nil,
      last_text: nil,
      pending_turns: 1,
      idle_timeout: Keyword.get(ctx.opts, :idle_timeout_ms, @idle_timeout_ms)
    })
  end

  # Resolve the child's working directory and refuse to spawn into one
  # that is not there: Erlang's child setup reports a failed `chdir` by
  # exiting with the raw errno -- `{"", 2}` for ENOENT, `{"", 20}` for
  # ENOTDIR -- indistinguishable from the CLI's own exit status, with no
  # output to explain it (#3989; the Cmd shape fixed in #3985).
  #
  # The default is never File.cwd!(): the OS cwd is BEAM-global and
  # cell-movable, so a child spawned without :cwd would inherit wherever
  # some other session last wandered (#3902). The boot-time capture is
  # stable.
  @spec validate_cwd!(keyword()) :: binary()
  defp validate_cwd!(opts) do
    {cwd, hint} =
      case Keyword.fetch(opts, :cwd) do
        {:ok, cwd} -> {cwd, ""}
        :error -> {IxMcp.Cmd.launch_cwd(), " (session launch dir deleted?)"}
      end

    case File.stat(cwd) do
      {:ok, %File.Stat{type: :directory}} ->
        cwd

      {:ok, %File.Stat{type: other}} ->
        raise ArgumentError, "cwd #{cwd} is not a directory (#{other})"

      {:error, :enoent} ->
        raise ArgumentError, "cwd #{cwd} does not exist#{hint}"

      {:error, reason} ->
        raise ArgumentError, "cwd #{cwd} is not usable: #{:file.format_error(reason)}"
    end
  end

  defp open_port(spec, cwd) do
    {exe, args} =
      case spec.stdin do
        :stream ->
          {spec.exe, spec.args}

        # codex blocks reading a piped stdin; hand it a closed one.
        :closed ->
          {"/bin/sh", ["-c", ~S(exec "$0" "$@" < /dev/null), spec.exe | spec.args]}
      end

    Port.open(
      {:spawn_executable, exe},
      [:binary, :exit_status, args: args, env: spec.env, cd: cwd]
    )
  end

  defp loop(state) do
    port = state.port

    receive do
      {^port, {:data, chunk}} ->
        {lines, buf} = take_lines(state.buf <> chunk)

        case Enum.reduce_while(lines, %{state | buf: buf}, fn line, acc ->
               handle_line(line, acc)
             end) do
          %{} = next -> continue_or_finish(next)
          {:done, result} -> result
        end

      {^port, {:exit_status, code}} ->
        exit_result(state, code)
    after
      state.idle_timeout ->
        safe_close(port)
        {:error, :idle_timeout}
    end
  end

  # A turn ended. Claude backends drain the mailbox here: queued lead
  # messages become new user turns on the same process. When nothing is
  # queued the run is over.
  defp continue_or_finish(%{final: nil} = state), do: loop(state)

  defp continue_or_finish(%{pending_turns: 0, stdin: :stream} = state) do
    {:ok, %{messages: msgs}} = AgentHarness.checkpoint(state.ctx.harness, state.ctx.agent_id)

    case msgs do
      [] ->
        safe_close(state.port)
        state.final

      msgs ->
        Enum.each(msgs, fn m -> inject_user(state.port, "[message from #{m.from}] #{m.text}") end)
        loop(%{state | final: nil, pending_turns: length(msgs)})
    end
  end

  defp continue_or_finish(%{pending_turns: 0} = state) do
    safe_close(state.port)
    state.final
  end

  defp continue_or_finish(state), do: loop(state)

  # The validate/spawn gap (TOCTOU): a cwd deleted after validation still
  # produces the bare-errno exit. A zero status means `chdir` succeeded,
  # and a run that reached a final result speaks for itself, so only a
  # nonzero exit with no final and the directory now gone is ambiguous --
  # raise rather than report a status that may be errno, not the CLI's.
  defp exit_result(%{final: nil, cwd: cwd}, code) when code != 0 do
    if File.dir?(cwd) do
      {:error, {:exit_status, code}}
    else
      raise "cwd #{cwd} no longer exists; exit #{code} may be the " <>
              "raw chdir errno rather than the CLI's own status"
    end
  end

  defp exit_result(%{final: nil}, code), do: {:error, {:exit_status, code}}
  defp exit_result(%{final: final}, _code), do: final

  defp take_lines(buf) do
    parts = String.split(buf, "\n")
    {rest, lines} = List.pop_at(parts, -1)
    {Enum.reject(lines, &(&1 == "")), rest || ""}
  end

  defp handle_line(line, state) do
    case JSON.decode(line) do
      {:ok, event} ->
        dispatch(state.backend, event, state)

      {:error, _reason} ->
        # Interleaved non-JSON noise (CLI notices); visible, never fatal.
        Events.record(state.ctx.agent_id, :noise, %{text: String.slice(line, 0, 200)})
        {:cont, state}
    end
  end

  # -- claude / kimi stream-json --

  defp dispatch(backend, event, state) when backend in [:claude, :kimi] do
    id = state.ctx.agent_id

    case event do
      %{"type" => "system", "subtype" => "init", "session_id" => sid} ->
        Events.put_session(id, sid)
        Events.record(id, :init, %{session: sid})
        {:cont, state}

      %{"type" => "system", "subtype" => subtype} ->
        Events.record(id, :meta, %{subtype: subtype})
        {:cont, state}

      %{"type" => "assistant", "message" => %{"content" => blocks}} when is_list(blocks) ->
        Enum.each(blocks, &record_block(id, &1))
        {:cont, state}

      %{"type" => "user", "message" => %{"content" => blocks}} when is_list(blocks) ->
        if Enum.any?(blocks, &match?(%{"type" => "tool_result"}, &1)) do
          Events.record(id, :tool_result, %{})
          {:cont, checkpoint_inject(state)}
        else
          {:cont, state}
        end

      %{"type" => "result"} = result ->
        finish_turn(state, result)

      _other ->
        Events.record(id, :meta, %{})
        {:cont, state}
    end
  end

  # -- codex exec --json --

  defp dispatch(:codex, event, state) do
    id = state.ctx.agent_id

    case event do
      %{"type" => "thread.started", "thread_id" => tid} ->
        Events.put_session(id, tid)
        Events.record(id, :init, %{session: tid})
        {:cont, state}

      %{"type" => "item.completed", "item" => item} ->
        codex_item(id, item, state)

      %{"type" => "turn.completed"} = turn ->
        add_usage(state, codex_tokens(turn))
        Events.record(id, :result, %{})
        safe_close(state.port)
        {:halt, {:done, {:ok, state.last_text || ""}}}

      %{"type" => "turn.failed"} = turn ->
        message = get_in(turn, ["error", "message"]) || "turn failed"
        Events.record(id, :error, %{snippet: String.slice(message, 0, 300)})
        safe_close(state.port)
        {:halt, {:done, {:error, message}}}

      %{"type" => "error", "message" => message} ->
        Events.record(id, :error, %{snippet: String.slice(message, 0, 300)})
        {:cont, state}

      _other ->
        {:cont, state}
    end
  end

  defp codex_item(id, %{"type" => "agent_message", "text" => text}, state) do
    Events.record(id, :text, %{snippet: String.slice(text, 0, 300)})
    {:cont, %{state | last_text: text}}
  end

  defp codex_item(id, %{"type" => "command_execution"} = item, state) do
    Events.record(id, :tool_use, %{
      tool: "command",
      snippet: String.slice(item["command"] || "", 0, 200)
    })

    {:cont, state}
  end

  defp codex_item(id, %{"type" => "error", "message" => message}, state) do
    Events.record(id, :meta, %{subtype: "error_item", snippet: String.slice(message, 0, 200)})
    {:cont, state}
  end

  defp codex_item(id, %{"type" => other}, state) do
    Events.record(id, :tool_use, %{tool: other})
    {:cont, state}
  end

  # An item without a "type" used to fall through dispatch's catch-all.
  defp codex_item(_id, _item, state), do: {:cont, state}

  defp record_block(id, %{"type" => "text", "text" => text}) do
    Events.record(id, :text, %{snippet: String.slice(text, 0, 300)})
  end

  defp record_block(id, %{"type" => "tool_use", "name" => name}) do
    Events.record(id, :tool_use, %{tool: name})
  end

  defp record_block(id, %{"type" => "thinking"}) do
    Events.record(id, :thinking, %{})
  end

  defp record_block(_id, _block), do: :ok

  defp finish_turn(state, result) do
    id = state.ctx.agent_id
    add_usage(state, claude_tokens(result))
    Events.record(id, :result, %{is_error: result["is_error"] == true})

    final =
      if result["is_error"] == true do
        {:error, result["result"] || "result error"}
      else
        {:ok, result["result"] || ""}
      end

    {:cont, %{state | final: final, pending_turns: state.pending_turns - 1}}
  end

  defp checkpoint_inject(%{stdin: :stream} = state) do
    {:ok, %{messages: msgs}} = AgentHarness.checkpoint(state.ctx.harness, state.ctx.agent_id)
    Enum.each(msgs, fn m -> inject_user(state.port, "[message from #{m.from}] #{m.text}") end)
    %{state | pending_turns: state.pending_turns + length(msgs)}
  end

  defp checkpoint_inject(state), do: state

  defp add_usage(state, tokens) do
    # Budget exhaustion surfaces at the next turn boundary as an error to
    # the lead; mid-stream the tokens are already spent.
    _result = AgentHarness.add_usage(state.ctx.harness, state.ctx.agent_id, tokens)
    :ok
  end

  defp claude_tokens(result) do
    usage = result["usage"] || %{}
    (usage["input_tokens"] || 0) + (usage["output_tokens"] || 0)
  end

  defp codex_tokens(turn) do
    usage = turn["usage"] || %{}
    (usage["input_tokens"] || 0) + (usage["output_tokens"] || 0)
  end

  defp inject_user(port, text) do
    line =
      JSON.encode!(%{
        "type" => "user",
        "message" => %{"role" => "user", "content" => [%{"type" => "text", "text" => text}]}
      })

    Port.command(port, [line, "\n"])
  end

  defp safe_close(port) do
    if Port.info(port), do: Port.close(port)
  catch
    # The port died between info and close; the process exit is the point.
    :error, :badarg -> :ok
  end
end
