defmodule UnibindConformanceTest do
  # Not async: the NIF exposes process-global counters
  # (cancelled_count/dropped_sessions) that concurrent tests would race.
  use ExUnit.Case, async: false

  alias UnibindConformance, as: Conf
  alias UnibindConformance.{ConformanceError, Native, Sample}

  # Poll `fun` until true or `timeout_ms` elapses; cancellation and GC are
  # asynchronous on the runtime/BEAM side, so observables need a grace window.
  defp eventually(fun, timeout_ms \\ 2_000) do
    deadline = System.monotonic_time(:millisecond) + timeout_ms
    poll(fun, deadline)
  end

  defp poll(fun, deadline) do
    cond do
      fun.() ->
        true

      System.monotonic_time(:millisecond) > deadline ->
        false

      true ->
        Process.sleep(20)
        poll(fun, deadline)
    end
  end

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
      Process.exit(pid, :kill)

      assert eventually(fn -> Conf.cancelled_count() >= baseline + 1 end),
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
