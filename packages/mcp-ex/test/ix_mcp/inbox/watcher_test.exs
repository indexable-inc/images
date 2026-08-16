defmodule IxMcp.Inbox.WatcherTest do
  use ExUnit.Case, async: false

  import IxMcpTest.Eventually

  alias IxMcp.Inbox.Watcher
  alias IxMcp.MCP.Notifier

  @source "fixture"

  # A scripted source: each sweep pops one reply, so a test can spell out a
  # sequence (a hit, a failure, a quiet window) with no network and no clock
  # games. The last reply repeats once the script runs dry, which is what
  # makes "and then it stops announcing" assertable. Every fetch records the
  # lower bound it was given, so the watermark is observable rather than
  # inferred from timing.
  defmodule Scripted do
    @behaviour IxMcp.Inbox.Source

    @impl true
    def init(opts) do
      case Keyword.get(opts, :script) do
        nil -> :ignore
        agent -> {:ok, %{agent: agent}}
      end
    end

    @impl true
    def label, do: "fixture"

    @impl true
    def default_interval_ms, do: 10

    @impl true
    def fetch(state, since, limit) do
      reply =
        Agent.get_and_update(state.agent, fn recorded ->
          {reply, rest} =
            case recorded.script do
              [last] -> {last, [last]}
              [head | tail] -> {head, tail}
              [] -> {{:ok, [], false}, []}
            end

          {reply, %{recorded | script: rest, calls: recorded.calls ++ [{since, limit}]}}
        end)

      case reply do
        {:ok, items, more?} -> {:ok, items, more?, state}
        {:error, detail} -> {:error, detail}
      end
    end
  end

  # A renderer that is not the chat one, so "the loop used the source's
  # renderer" is observable rather than inferred: it prints a shape
  # IxMcp.Inbox.Announce cannot produce and tags its own meta key.
  defmodule Painted do
    @behaviour IxMcp.Inbox.Renderer

    alias IxMcp.MCP.Notifier

    @impl true
    def announce(source, item) do
      Notifier.channel("painted #{item.id}", %{
        "source" => source,
        "id" => item.id,
        "renderer" => "painted"
      })
    end

    @impl true
    def announce_overflow(source, shown) do
      Notifier.channel("painted overflow #{shown}", %{
        "source" => source,
        "overflow" => "true",
        "renderer" => "painted"
      })
    end
  end

  # Same script mechanics, plus the two optional callbacks.
  defmodule ScriptedPainted do
    @behaviour IxMcp.Inbox.Source

    alias IxMcp.Inbox.WatcherTest.Painted
    alias IxMcp.Inbox.WatcherTest.Scripted

    @impl true
    def init(opts), do: Scripted.init(opts)

    @impl true
    def label, do: "fixture"

    @impl true
    def default_interval_ms, do: 10

    @impl true
    def renderer, do: Painted

    @impl true
    def initial_backfill_s, do: 1_800

    @impl true
    def fetch(state, since, limit), do: Scripted.fetch(state, since, limit)
  end

  defp item(id) do
    %{id: id, platform: "Signal", sender: "Fixture Sender", context: nil, preview: "text #{id}"}
  end

  defp script(replies) do
    {:ok, agent} = Agent.start_link(fn -> %{script: replies, calls: []} end)
    agent
  end

  defp calls(agent), do: Agent.get(agent, & &1.calls)

  defp start_watcher(agent, opts) do
    start_supervised!(
      {Watcher,
       Keyword.merge(
         [
           source: Scripted,
           script: agent,
           name: nil,
           interval_ms: 10,
           transports: fn -> 1 end
         ],
         opts
       )}
    )
  end

  defp await_ids(count) do
    for _each <- 1..count do
      assert_receive {:mcp_send,
                      %{
                        "method" => "notifications/claude/channel",
                        "params" => %{"meta" => %{"source" => @source} = meta}
                      }},
                     2_000

      meta["id"]
    end
  end

  setup do
    Notifier.register(self())
    _state = :sys.get_state(Notifier)
    :ok
  end

  test "a source with no credential never starts the watcher" do
    assert :ignore = Watcher.start_link(source: Scripted, name: nil)
  end

  test "each message announces once, and the overlap re-read is deduped" do
    # The same page forever: the first sweep must announce both messages in
    # order, every later sweep must swallow them.
    agent = script([{:ok, [item("a"), item("b")], false}])
    start_watcher(agent, [])

    assert await_ids(2) == ["a", "b"]

    refute_receive {:mcp_send, %{"params" => %{"meta" => %{"source" => @source}}}}, 200
  end

  test "with no transport attached nothing is fetched, so the watermark cannot advance" do
    agent = script([{:ok, [item("held")], false}])
    attached = script([])
    {:ok, gate} = Agent.start_link(fn -> 0 end)

    start_watcher(agent, transports: fn -> Agent.get(gate, & &1) end)

    # A channel event has no outbox and no replay: announcing here would drop
    # the message on the floor. So the sweep must not even ask the source.
    refute_receive {:mcp_send, %{"params" => %{"meta" => %{"source" => @source}}}}, 150
    assert calls(agent) == []
    assert calls(attached) == []

    # The other verdict of the same gate: once a session is back, the message
    # that arrived while nobody listened still lands.
    Agent.update(gate, fn _zero -> 1 end)
    assert await_ids(1) == ["held"]
  end

  test "a failed sweep loses nothing: the next one re-reads the same window" do
    agent = script([{:error, "fixture outage"}, {:ok, [item("survived")], false}])
    start_watcher(agent, [])

    assert await_ids(1) == ["survived"]

    # The watermark is the proof, not the announcement: both sweeps asked
    # from the same lower bound, so the failure widened nothing and skipped
    # nothing.
    [{first_since, _limit}, {second_since, _limit2} | _rest] = calls(agent)
    assert DateTime.compare(first_since, second_since) == :eq
  end

  test "overflow is announced with the batch, and not again on an idle sweep" do
    agent = script([{:ok, [item("only")], true}])
    start_watcher(agent, [])

    assert await_ids(1) == ["only"]

    assert_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"source" => @source, "overflow" => "true"}}}},
                   2_000

    # `more?` stays true while the window is busy; an overflow line per idle
    # sweep would be exactly the noise this feed tries not to be.
    refute_receive {:mcp_send, %{"params" => %{"meta" => %{"overflow" => "true"}}}}, 200
  end

  test "the lower bound is widened by the overlap but floored by the backfill cap" do
    agent = script([{:ok, [], false}])
    start_watcher(agent, overlap_s: 3_600, max_backfill_s: 5)

    recorded = eventually(fn -> if calls(agent) == [], do: nil, else: calls(agent) end)
    [{since, _limit} | _rest] = recorded
    age = DateTime.diff(DateTime.utc_now(), since)

    # Without the cap the overlap would have asked for the last hour, which
    # is how a re-attached session opens with a wall of old chat.
    assert age <= 10, "expected the backfill cap to floor the window, got #{age}s"
  end

  test "a source that names a renderer gets its lines, not the chat ones" do
    agent = script([{:ok, [item("painted-me")], true}])

    start_supervised!(
      {Watcher,
       source: ScriptedPainted, script: agent, name: nil, interval_ms: 10, transports: fn -> 1 end}
    )

    assert_receive {:mcp_send,
                    %{
                      "params" => %{
                        "content" => "painted painted-me",
                        "meta" => %{"renderer" => "painted"}
                      }
                    }},
                   2_000

    # The overflow line goes through the same renderer, or a busy window would
    # narrate itself in a voice the rest of the feed does not use.
    assert_receive {:mcp_send,
                    %{
                      "params" => %{
                        "content" => "painted overflow 20",
                        "meta" => %{"renderer" => "painted", "overflow" => "true"}
                      }
                    }},
                   2_000
  end

  test "a source that asks for a first-sweep backfill gets one, and the chat feeds do not" do
    painted = script([{:ok, [], false}])
    plain = script([{:ok, [], false}])

    start_supervised!(
      {Watcher,
       source: ScriptedPainted,
       script: painted,
       name: nil,
       interval_ms: 10,
       transports: fn -> 1 end}
    )

    start_supervised!(
      {Watcher,
       source: Scripted, script: plain, name: nil, interval_ms: 10, transports: fn -> 1 end}
    )

    [{painted_since, _limit} | _rest] = eventually(fn -> presence(painted) end)
    [{plain_since, _limit2} | _rest2] = eventually(fn -> presence(plain) end)

    # 1800s asked for, floored by the default 900s backfill cap: a CI run
    # outlives a kernel restart, so the first sweep has to look back, and the
    # cap is what keeps "look back" from becoming "replay the day".
    painted_age = DateTime.diff(DateTime.utc_now(), painted_since)
    assert painted_age >= 800, "expected a backfilled first window, got #{painted_age}s"

    # The chat feeds are unchanged: their first window is the overlap only.
    plain_age = DateTime.diff(DateTime.utc_now(), plain_since)
    assert plain_age <= 120, "expected no backfill for a chat feed, got #{plain_age}s"
  end

  defp presence(agent), do: if(calls(agent) == [], do: nil, else: calls(agent))
end
