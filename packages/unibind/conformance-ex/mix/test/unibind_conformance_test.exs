# A consumer that never blocks: every stream message arrives through
# `handle_info/2`, which is the shape a real GenServer needs and the reason
# the `Enumerable` form alone was not enough (its `receive` owns the
# process until the producer answers).
defmodule UnibindConformanceTest.Collector do
  @moduledoc false
  use GenServer

  alias UnibindConformance, as: Conf

  def start_link(n), do: GenServer.start_link(__MODULE__, n)

  @doc "The collected items, once the stream has finished."
  def take(pid), do: GenServer.call(pid, :take, 5_000)

  @doc "Round-trip a call while the stream is still running."
  def ping(pid), do: GenServer.call(pid, :ping, 5_000)

  @impl true
  def init(n) do
    stream = Conf.count_stream(n)
    :ok = Conf.stream_demand(stream, 1)
    {:ok, %{stream: stream, items: [], waiting: nil, done: false}}
  end

  @impl true
  def handle_call(:ping, _from, state), do: {:reply, :pong, state}

  def handle_call(:take, _from, %{done: true} = state) do
    {:reply, Enum.reverse(state.items), state}
  end

  def handle_call(:take, from, state), do: {:noreply, %{state | waiting: from}}

  @impl true
  def handle_info(message, state) do
    case Conf.stream_message(state.stream, message) do
      {:item, item} ->
        :ok = Conf.stream_demand(state.stream, 1)
        {:noreply, %{state | items: [item | state.items]}}

      :done ->
        if state.waiting, do: GenServer.reply(state.waiting, Enum.reverse(state.items))
        {:noreply, %{state | done: true, waiting: nil}}

      :nomatch ->
        {:noreply, state}
    end
  end
end

defmodule UnibindConformanceTest do
  # Not async: the NIF exposes process-global counters
  # (cancelled_count/dropped_sessions) that concurrent tests would race.
  use ExUnit.Case, async: false

  alias UnibindConformance, as: Conf
  alias UnibindConformance.{ConformanceError, Native, Sample, StreamHandle}
  alias UnibindConformanceTest.Collector

  import UnibindTest.Eventually

  defp sample(id) do
    %Sample{
      id: id,
      name: "sample-#{id}",
      ratio: 0.5,
      tags: ["conformance"],
      home: nil
    }
  end

  describe "echo round-trips" do
    test "bool round-trips" do
      assert Conf.echo_bool(true) == true
      assert Conf.echo_bool(false) == false
    end

    test "i64 round-trips, including negatives" do
      assert Conf.echo_int(0) == 0
      assert Conf.echo_int(-9_007_199_254_740_993) == -9_007_199_254_740_993
    end

    test "u32 round-trips" do
      assert Conf.echo_uint(4_294_967_295) == 4_294_967_295
    end

    test "f64 round-trips" do
      assert Conf.echo_float(1.5) == 1.5
    end

    test "String round-trips utf-8" do
      assert Conf.echo_str("hello \u00fc\u4e16\u754c") == "hello \u00fc\u4e16\u754c"
    end

    test "Option<String> round-trips nil and value" do
      assert Conf.echo_option(nil) == nil
      assert Conf.echo_option("present") == "present"
    end

    test "Vec<i64> round-trips" do
      assert Conf.echo_vec([1, -2, 3]) == [1, -2, 3]
    end

    test "Map<String, i64> round-trips" do
      assert Conf.echo_map(%{"a" => 1, "b" => -2}) == %{"a" => 1, "b" => -2}
    end

    test "record round-trips as %UnibindConformance.Sample{} struct identity" do
      input = %Sample{id: 7, name: "seven", ratio: 0.25, tags: ["a", "b"], home: "/tmp"}
      assert %Sample{} = echoed = Conf.echo_record(input)
      assert echoed == input
    end

    test "nested Vec<record> round-trips" do
      inputs = [sample(1), sample(2)]
      assert Conf.echo_records(inputs) == inputs
    end
  end

  describe "binary payloads" do
    # NUL and high bytes in every fixture: a UTF-8 or charlist path would
    # mangle both, so these assertions fail loudly if bytes ever regress to
    # rustler's element-wise Vec<u8> codec (a list of integers).
    @blob <<0, 255, 254, 128, 1>>

    test "&[u8] round-trips as a binary, not a list of integers" do
      assert echoed = Conf.echo_bytes(@blob)
      assert is_binary(echoed)
      assert echoed == @blob
    end

    test "Vec<u8> round-trips" do
      assert Conf.echo_bytes_owned(@blob) == @blob
    end

    test "empty binaries round-trip" do
      assert Conf.echo_bytes(<<>>) == <<>>
      assert Conf.echo_bytes_owned(<<>>) == <<>>
      assert Conf.bytes_len(<<>>) == 0
    end

    test "the whole binary arrives: length counts bytes, not codepoints" do
      assert Conf.bytes_len(@blob) == 5
      # 3 bytes of UTF-8, 1 codepoint: a text codec would answer 1.
      assert Conf.bytes_len("世") == 3
    end

    test "a list is not a binary: the decoder rejects it" do
      assert_raise ArgumentError, fn -> Conf.echo_bytes([0, 255, 254]) end
    end

    test "Option<Vec<u8>> round-trips nil and value" do
      assert Conf.echo_bytes_option(nil) == nil
      assert Conf.echo_bytes_option(@blob) == @blob
    end

    test "Option<&[u8]> round-trips nil and value" do
      assert Conf.echo_bytes_option_ref(nil) == nil
      assert Conf.echo_bytes_option_ref(@blob) == @blob
    end

    test "Vec<Vec<u8>> round-trips as a list of binaries" do
      inputs = [@blob, <<>>, <<7>>]
      assert echoed = Conf.echo_bytes_list(inputs)
      assert Enum.all?(echoed, &is_binary/1)
      assert echoed == inputs
    end

    test "HashMap<String, Vec<u8>> round-trips binary values" do
      inputs = %{"a" => @blob, "b" => <<>>}
      assert Conf.echo_bytes_map(inputs) == inputs
    end

    test "a blocking (DirtyIo) NIF carries binaries" do
      assert Conf.blocking_bytes(@blob) == @blob
    end

    test "binaries ride inside {:ok, _} and the error variant still works" do
      assert Conf.maybe_bytes(false) == {:ok, <<0, 255, 128>>}
      assert {:error, %ConformanceError{variant: :gone}} = Conf.maybe_bytes(true)
    end

    test "async replies carry binaries" do
      assert Conf.echo_bytes_async(@blob) == @blob
    end

    test "raw async contract: the reply term is a binary" do
      ref = make_ref()
      _inflight = Native.echo_bytes_async(ref, @blob)
      assert_receive {:unibind, ^ref, {:ok, reply}}, 1_000
      assert is_binary(reply)
      assert reply == @blob
    end

    test "streams yield binaries" do
      assert Enum.to_list(Conf.count_blobs(3)) == [
               <<0, 255, ?0>>,
               <<0, 255, ?1>>,
               <<0, 255, ?2>>
             ]
    end
  end

  describe "error terms" do
    test "Ok crosses as {:ok, value}" do
      assert Conf.maybe_fail(false) == {:ok, 42}
    end

    test "Err crosses as {:error, %ConformanceError{}} with variant atom and message" do
      assert {:error, %ConformanceError{variant: :deliberate, message: message}} =
               Conf.maybe_fail(true)

      assert message == "conformance deliberate failure"
    end

    test "each variant maps to its own atom (:gone)" do
      assert {:error, %ConformanceError{variant: :gone}} = Conf.lost()
    end
  end

  describe "blocking (DirtyIo) scheduling" do
    test "a #[unibind(blocking)] NIF runs and returns" do
      assert Conf.blocking_sleep_ms(50) == :ok
    end
  end

  describe "async reply" do
    test "raw contract: Native async NIF replies {:unibind, ref, {:ok, value}}" do
      ref = make_ref()
      _inflight = Native.echo_async(ref, "hello")
      assert_receive {:unibind, ^ref, {:ok, "hello"}}, 1_000
    end

    test "wrapper contract: echo_async/1 blocks on the reply and returns the value" do
      assert Conf.echo_async("hello") == "hello"
    end

    test "wrapper contract: throwing async fn returns {:ok, value} | {:error, error}" do
      assert Conf.maybe_fail_async(false) == {:ok, 7}

      assert {:error, %ConformanceError{variant: :deliberate}} =
               Conf.maybe_fail_async(true)
    end
  end

  describe "caller-exit cancellation" do
    test "a caller exiting mid-call drops the in-flight future (cancelled_count)" do
      baseline = Conf.cancelled_count()
      started = Conf.started_count()
      parent = self()

      pid =
        spawn(fn ->
          ref = make_ref()
          _inflight = Native.slow(ref, 600_000)
          send(parent, :started)

          receive do
            :never -> :ok
          end
        end)

      assert_receive :started, 1_000

      # Kill only once slow's body is running: the NIF returning proves the
      # future was spawned, not polled, and an abort that lands before the
      # first poll drops it with the cancel guard never armed, so
      # cancelled_count would stay flat forever (index#3974; main runs
      # 29893447304 and 29895946094 lost that race under CI load).
      assert eventually(fn -> Conf.started_count() >= started + 1 end, 15_000),
             "slow/1 never started executing"

      Process.exit(pid, :kill)

      # 15s, not the 2s default: this ran on CI hosts saturated by parallel
      # nix builds, where caller-exit cancellation took over 2s to surface
      # (main run 29889948013 missed the window by a hair).
      assert eventually(fn -> Conf.cancelled_count() >= baseline + 1 end, 15_000),
             "cancelled_count never reached baseline + 1"
    end

    test "a completed call does not count as cancelled" do
      baseline = Conf.cancelled_count()
      assert Conf.slow(10) == 10
      # Grace period: a false increment would arrive asynchronously.
      Process.sleep(100)
      assert Conf.cancelled_count() == baseline
    end
  end

  describe "resource destructor on GC" do
    test "session methods share state through the handle" do
      session = Conf.Session.new(3)
      assert Conf.Session.get(session) == 3
      assert Conf.Session.add(session, 4) == 7
      assert Conf.Session.get(session) == 7
    end

    test "process death frees the resource: Drop runs (dropped_sessions)" do
      baseline = Conf.dropped_sessions()
      parent = self()

      pid =
        spawn(fn ->
          session = Conf.Session.new(1)
          send(parent, {:value, Conf.Session.get(session)})
        end)

      assert_receive {:value, 1}, 1_000
      refute Process.alive?(pid)

      assert eventually(fn -> Conf.dropped_sessions() >= baseline + 1 end),
             "dropped_sessions never reached baseline + 1"
    end

    test "in-process :erlang.garbage_collect after dropping the last ref runs Drop" do
      baseline = Conf.dropped_sessions()
      make = fn -> Conf.Session.new(5) end
      make.()
      :erlang.garbage_collect()

      assert eventually(fn -> Conf.dropped_sessions() >= baseline + 1 end),
             "dropped_sessions never moved after garbage_collect"
    end
  end

  describe "streams" do
    test "count/1 consumes lazily as an Enumerable and yields 0..n-1" do
      assert Enum.to_list(Conf.count(5)) == [0, 1, 2, 3, 4]
    end

    test "record streams yield structs" do
      assert Enum.to_list(Conf.count_samples(2)) == [
               %Sample{id: 0, name: "sample-0", ratio: 0.5, tags: ["conformance"], home: nil},
               %Sample{id: 1, name: "sample-1", ratio: 0.5, tags: ["conformance"], home: nil}
             ]
    end

    test "demand convention: without demand no {:unibind_stream, ...} item arrives" do
      ref = make_ref()
      _handle = Native.count(ref, 3)
      refute_receive {:unibind_stream, ^ref, _}, 100
    end

    test "count_stream/1 returns a handle, not an Enumerable" do
      assert %StreamHandle{ref: ref, handle: handle} = Conf.count_stream(3)
      assert is_reference(ref)
      assert is_reference(handle)
    end

    test "stream_demand/2 and stream_message/2 drive the handle without a blocking receive" do
      stream = Conf.count_stream(2)
      refute_receive {:unibind_stream, _, _}, 100

      assert :ok = Conf.stream_demand(stream, 1)
      assert_receive item_message, 1_000
      assert Conf.stream_message(stream, item_message) == {:item, 0}

      assert :ok = Conf.stream_demand(stream, 2)
      assert_receive second, 1_000
      assert Conf.stream_message(stream, second) == {:item, 1}
      assert_receive done, 1_000
      assert Conf.stream_message(stream, done) == :done
    end

    test "stream_message/2 answers :nomatch for a foreign message" do
      stream = Conf.count_stream(1)
      assert Conf.stream_message(stream, {:tcp, self(), "data"}) == :nomatch
      assert Conf.stream_message(stream, {:unibind_stream, make_ref(), :done}) == :nomatch
    end

    test "a GenServer consumes a stream from handle_info and stays responsive" do
      {:ok, pid} = Collector.start_link(4)
      # The point of the handle API: the server answers other calls while
      # the stream is still in flight, which the Enumerable form cannot do.
      assert Collector.ping(pid) == :pong
      assert Collector.take(pid) == [0, 1, 2, 3]
    end

    test "binary stream items cross the handle API too" do
      stream = Conf.count_blobs_stream(1)
      assert :ok = Conf.stream_demand(stream, 1)
      assert_receive message, 1_000
      assert {:item, blob} = Conf.stream_message(stream, message)
      assert blob == <<0, 255, ?0>>
    end

    test "demand convention: one credit, one {:unibind_stream, ref, {:item, _}}; :done after the end" do
      ref = make_ref()
      handle = Native.count(ref, 3)

      Native.unibind_demand(handle, 1)
      assert_receive {:unibind_stream, ^ref, {:item, 0}}, 1_000
      refute_receive {:unibind_stream, ^ref, _}, 100

      Native.unibind_demand(handle, 2)
      assert_receive {:unibind_stream, ^ref, {:item, 1}}, 1_000
      assert_receive {:unibind_stream, ^ref, {:item, 2}}, 1_000
      refute_receive {:unibind_stream, ^ref, _}, 100

      Native.unibind_demand(handle, 1)
      assert_receive {:unibind_stream, ^ref, :done}, 1_000
    end
  end
end
