defmodule IxMcp.FleetWatchOfferTest do
  use ExUnit.Case, async: false

  alias IxMcp.Fleet.WatchOffer

  # async: false: the once-per-boot latch is a persistent term.

  setup do
    WatchOffer.reset()
    :ok
  end

  defp red_snapshot do
    %{disk: %{state: :red, since: 0, detail: []}, calm: %{state: :green, since: 0, detail: nil}}
  end

  defp offer(snapshot, ask, start_watch) do
    parent = self()

    WatchOffer.maybe_offer(
      snapshot: fn -> snapshot end,
      ask: ask,
      start_watch: start_watch,
      done: fn -> send(parent, :offer_done) end
    )

    assert_receive :offer_done, 1_000
  end

  test "standing warnings plus a yes starts the watch, naming the source" do
    parent = self()

    offer(
      red_snapshot(),
      fn question, opts ->
        send(parent, {:asked, question, opts})
        {:ok, "yes"}
      end,
      fn label ->
        send(parent, {:started, label})
        {:ok, self()}
      end
    )

    assert_received {:asked, question, _opts}
    assert question =~ "disk"
    refute question =~ "calm"
    assert_received {:started, "elicited at connect"}
  end

  test "an all-green fleet asks nothing" do
    parent = self()
    green = %{calm: %{state: :green, since: 0, detail: nil}}

    offer(green, fn _q, _o -> send(parent, :asked) end, fn _l -> send(parent, :started) end)

    refute_received :asked
    refute_received :started
  end

  test "a no leaves the watch off" do
    parent = self()

    offer(red_snapshot(), fn _q, _o -> :declined end, fn _l -> send(parent, :started) end)

    refute_received :started
  end

  test "the offer happens once per boot, however many sessions connect" do
    parent = self()

    offer(red_snapshot(), fn _q, _o -> send(parent, :asked) && :declined end, fn _l -> :ok end)
    assert_received :asked

    # Second connect: latched, no task, no ask.
    WatchOffer.maybe_offer(
      snapshot: fn -> red_snapshot() end,
      ask: fn _q, _o -> send(parent, :asked_again) end,
      start_watch: fn _l -> :ok end,
      done: fn -> send(parent, :second_done) end
    )

    refute_receive :asked_again, 200
    refute_received :second_done
  end

  test "a client that cannot elicit is not a crash" do
    offer(red_snapshot(), fn _q, _o -> raise "method not found" end, fn _l -> :ok end)
    # Reaching here (done fired from the rescue) is the assertion.
  end
end
