defmodule IxMcp.Forge.RunsTest do
  @moduledoc false
  use ExUnit.Case, async: true

  alias IxMcp.Forge.Runs

  @local %{host: nil, dir: "/unused"}

  describe "run/3 stream separation" do
    # The land this module exists to serve was refused because jj's
    # "Concurrent modification detected, resolving automatically." arrived on
    # the same stream as a list of paths. Both halves are asserted: `parse:
    # true` must hide stderr, and the default must still show it, because the
    # default is what makes a failure's reason readable.
    test "parsed output carries stdout only" do
      assert {:ok, output} =
               Runs.run(@local, ["sh", "-c", "echo wanted; echo noise >&2"], parse: true)

      assert output == "wanted\n"
    end

    test "unparsed output still carries stderr, which is where failures explain themselves" do
      assert {:ok, output} = Runs.run(@local, ["sh", "-c", "echo wanted; echo noise >&2"], [])

      assert output =~ "wanted"
      assert output =~ "noise"
    end

    test "a parsed command that fails reports its stderr rather than swallowing it" do
      assert {:error, detail} =
               Runs.run(@local, ["sh", "-c", "echo out; echo why >&2; exit 3"], parse: true)

      assert detail =~ "why"
      refute detail =~ "ix-stderr-"
    end

    test "a parsed command's own output cannot be mistaken for the sentinel" do
      # The sentinel is invented per call, so text shaped like one is data.
      assert {:ok, output} =
               Runs.run(@local, ["sh", "-c", "echo 'ix-stderr-deadbeefdeadbeef'"], parse: true)

      assert output == "ix-stderr-deadbeefdeadbeef\n"
    end
  end

  describe "run_detached/3" do
    setup do
      state = Path.join(System.tmp_dir!(), "ix-detached-#{System.unique_integer([:positive])}")
      on_exit(fn -> File.rm_rf(state) end)
      {:ok, state: state}
    end

    test "a step's stdout comes back, and only its stdout", %{state: state} do
      assert {:ok, output} =
               Runs.run_detached(@local, ["sh", "-c", "echo wanted; echo noise >&2"],
                 state: state,
                 timeout_ms: 30_000
               )

      assert output == "wanted\n"
    end

    # The verdict leg. A detached step's exit code travels through a file
    # written by a nested shell, which is exactly the kind of plumbing that
    # silently reports 0 for everything; an executor that cannot say "failed"
    # would turn every failed land step into a land that continued.
    test "a step that exits non-zero is reported as failed, with its stderr", %{state: state} do
      assert {:error, detail} =
               Runs.run_detached(@local, ["sh", "-c", "echo out; echo why >&2; exit 3"],
                 state: state,
                 timeout_ms: 30_000
               )

      assert detail =~ "exited 3"
      assert detail =~ "why"
    end

    test "the step survives the launcher's exit code being 0", %{state: state} do
      # `false` after a successful launch: the rc file must describe the STEP,
      # not the shell that started it.
      assert {:error, detail} =
               Runs.run_detached(@local, ["false"], state: state, timeout_ms: 30_000)

      assert detail =~ "exited 1"
    end

    # A step still running at its deadline has an outcome nobody has read. That
    # is UNKNOWN, not failed: a caller told "failed" will retry an action that
    # may have succeeded, which for a submit means double-submitting.
    test "a step slower than its budget is UNKNOWN, not failed and not a success",
         %{state: state} do
      assert {:unknown, detail} =
               Runs.run_detached(@local, ["sh", "-c", "sleep 30"],
                 state: state,
                 timeout_ms: 1_000
               )

      assert detail =~ "did not finish"
    end

    # M1 + C5 in one: the evidence a failed land needs survives, and the file
    # the lost-acknowledgement probe keys on is written by the WRAPPER (cmd.sh
    # exists before nohup, so probing for it would call a dead launch "started").
    test "a failed step keeps its directory, and the wrapper records that it started",
         %{state: state} do
      assert {:error, detail} =
               Runs.run_detached(@local, ["sh", "-c", "echo why >&2; exit 4"],
                 state: state,
                 timeout_ms: 30_000
               )

      assert detail =~ "full output kept at"
      [dir] = Path.wildcard(Path.join(state, "ix-step-*"))
      assert File.exists?(Path.join(dir, "started"))
      assert File.read!(Path.join(dir, "err")) =~ "why"
      assert String.trim(File.read!(Path.join(dir, "rc"))) == "4"
    end

    test "output shaped like the step marker is data", %{state: state} do
      assert {:ok, output} =
               Runs.run_detached(@local, ["sh", "-c", "echo ix-step-deadbeefdeadbeef"],
                 state: state,
                 timeout_ms: 30_000
               )

      assert output == "ix-step-deadbeefdeadbeef\n"
    end

    test "no :state is refused rather than guessed at" do
      assert {:error, detail} = Runs.run_detached(@local, ["true"], [])
      assert detail =~ ":state"
    end

    test "the step directory is cleaned up once collected", %{state: state} do
      assert {:ok, _output} =
               Runs.run_detached(@local, ["true"], state: state, timeout_ms: 30_000)

      assert File.ls!(state) == []
    end
  end

  describe "detail/1 and the stage exit codes" do
    # Shape read off a LIVE failed record on 2026-08-13 (free text replaced,
    # paths sanitized): the lint stage hit its 5400-second budget, `timeout`
    # killed it with 124, and NO derivation is named because nothing got far
    # enough to fail. Without the code, that record is indistinguishable from a
    # change that broke the lint gate -- and the response to the two is
    # opposite: report the budget, or fix the change.
    @timed_out """
    ==== jj-forge gate v2 (lint + gate-eval + scoped build) ====
    log:    /fixture/logs/gate-20260101T000000Z-3.log
    stage lint: start (timeout 5400s)
    stage lint: FAIL rc=124 in 5401s

    --- nix error lines (last 8) ---
    error: interrupted by the user

    ==== gate verdict: FAIL ====
    stages:
      lint   FAIL rc=124                                    5401s
      fmt    not reached
    log: /fixture/logs/gate-20260101T000000Z-3.log
    """

    @real_red """
    ==== gate verdict: FAIL ====
    stages:
      lint   pass                                             34s
      incr   FAIL rc=1                                       307s
    log: /fixture/logs/gate-20260101T000000Z-4.log
    """

    test "a stage killed by its budget is reported with its code and named a timeout" do
      detail = Runs.detail(@timed_out)

      assert detail.failed_stages == ["lint"]
      assert detail.stage_failures == [%{stage: "lint", rc: 124, timed_out: true}]
      assert detail.failing_derivations == []
      assert detail.verdict_line == "==== gate verdict: FAIL ===="
    end

    test "an ordinary red carries its code and is NOT a timeout" do
      detail = Runs.detail(@real_red)

      assert detail.stage_failures == [%{stage: "incr", rc: 1, timed_out: false}]
      assert detail.failed_stages == ["incr"]
    end

    # The real shape of a budget-killed run, read off run 84a6f4e7e42 on
    # 2026-08-13 (free text and paths sanitized, stage lines verbatim). Three
    # things must come out right at once, and a reader that greps for FAIL gets
    # all three wrong: the fatal stage is `eval` and it is named with the WORD
    # TIMEOUT and no rc; `incr fallback rc=124` is the gate degrading to the
    # serial path and CONTINUING, so it is not a failure despite carrying 124;
    # and `fmt warn rc=1` is warn-only by design.
    @budget_killed """
    stage lint: pass (baseline-tolerated: fixture-tolerated-check) in 131s
    stage fmt: warn rc=1 in 21s
    stage incr: fallback rc=124 in 7200s: running the full serial eval instead
    stage eval: TIMEOUT after 3600s in 3602s
    ==== gate verdict: FAIL ====
    stages:
      fmt    warn rc=1                                      21s
      incr   fallback rc=124                                7200s
      eval   TIMEOUT after 3600s                            3602s
    log: /fixture/logs/gate-20260101T000000Z-5.log
    """

    test "a stage the gate reports as TIMEOUT is a failed stage, and a timeout" do
      detail = Runs.detail(@budget_killed)

      assert detail.failed_stages == ["eval"]
      assert detail.stage_failures == [%{stage: "eval", rc: nil, timed_out: true}]
    end

    test "a fallback carrying 124 is not a failed stage, and neither is a warn" do
      detail = Runs.detail(@budget_killed)

      refute "incr" in detail.failed_stages
      refute "fmt" in detail.failed_stages
    end

    test "a stage table with no code at all still names the stage" do
      detail = Runs.detail("stages:\n  incr   FAIL                          307s\n")

      assert detail.stage_failures == [%{stage: "incr", rc: nil, timed_out: false}]
    end

    test "a record with no log tail has an empty detail rather than a nil one" do
      detail = Runs.detail(nil)

      assert detail.stage_failures == []
      assert detail.failed_stages == []
    end
  end

  describe "parse_step/2 (the ssh-chatter law)" do
    # The poll's stdout does not begin where the poll SCRIPT begins: ssh writes
    # its own lines onto the same stream. A head-anchored "rc=" parse reads one
    # of those as "still running", so a finished step waits out its whole budget
    # and is then reported as a timeout. Both the chatter case and the genuinely
    # -running case are asserted, because a parser that always says done is just
    # as wrong.
    test "chatter before the fields does not hide a finished step" do
      step = "ix-step-abc123"
      chatter = "Warning: Permanently added 'host' (ED25519) to the list of known hosts.\n"

      assert {:done, 0, "the output\n", ""} =
               Runs.parse_step(chatter <> step <> "0\n" <> step <> "the output\n" <> step, step)
    end

    test "a non-zero code survives the chatter too" do
      step = "ix-step-abc123"

      assert {:done, 3, _out, "why\n"} =
               Runs.parse_step(
                 "banner text\n" <> step <> "3\n" <> step <> "" <> step <> "why\n",
                 step
               )
    end

    test "output with no fields is still running" do
      assert :running = Runs.parse_step("running\n", "ix-step-abc123")
    end

    test "a marker with an unreadable code is still running, never done" do
      step = "ix-step-abc123"

      assert :running =
               Runs.parse_step(step <> "not-a-number" <> step <> "x" <> step <> "y", step)
    end
  end

  describe "put_file/3" do
    setup do
      root = Path.join(System.tmp_dir!(), "ix-put-#{System.unique_integer([:positive])}")
      on_exit(fn -> File.rm_rf(root) end)
      {:ok, root: root}
    end

    test "the bytes land, parents and all", %{root: root} do
      path = Path.join([root, "deep", "deeper", "file.ex"])

      assert :ok = Runs.put_file(@local, path, "defmodule X do\nend\n")
      assert File.read!(path) == "defmodule X do\nend\n"
    end

    # The size that killed land attempt 4 was 48 KB, so the test uses more than
    # MAX_ARG_STRLEN: any implementation that goes back to carrying the body in
    # argv fails here instead of in production on the one file that is too big.
    test "a body larger than a single argv string is fine", %{root: root} do
      path = Path.join(root, "big.txt")
      body = String.duplicate("abcdefgh", 40_000)

      assert byte_size(body) > 131_072
      assert :ok = Runs.put_file(@local, path, body)
      assert File.read!(path) == body
    end

    test "a path a remote shell or sftp could reinterpret is refused" do
      for hostile <- [
            "relative/path",
            "/tmp/with space",
            "/tmp/$(id)",
            "/tmp/`id`",
            "/tmp/quote'name",
            "/tmp/semi;rm",
            "/tmp/star*"
          ] do
        assert {:error, detail} = Runs.put_file(@local, hostile, "x")
        assert detail =~ "not a safe absolute path"
      end
    end

    # Traversal is not an injection but it IS a wrong write, and the trailing
    # slash is the quiet one: scp copies INTO a directory and names the file
    # after the local temp, so the write would report :ok having put the bytes
    # somewhere nobody asked for.
    test "traversal segments and a trailing slash are refused" do
      assert {:error, up} = Runs.put_file(@local, "/root/state/../../etc/cron.d/evil", "x")
      assert up =~ "segment"

      assert {:error, dot} = Runs.put_file(@local, "/root/./state/x", "x")
      assert dot =~ "segment"

      assert {:error, slash} = Runs.put_file(@local, "/root/state/", "x")
      assert slash =~ "trailing slash"
    end

    test "an unwritable destination is an error, not a silent success" do
      assert {:error, detail} = Runs.put_file(@local, "/definitely-not-writable/x", "x")
      assert detail =~ "/definitely-not-writable/x"
    end
  end

  describe "parse_target/1" do
    test "the three accepted shapes" do
      assert {:ok, %{host: nil, dir: "/runs"}} = Runs.parse_target("/runs")
      assert {:ok, %{host: "host", dir: "/runs"}} = Runs.parse_target("host:/runs")
      assert {:ok, %{host: "root@host", dir: "/runs"}} = Runs.parse_target("root@host:/runs")
    end

    test "anything a remote shell could reinterpret is refused, not sanitized" do
      for hostile <- [
            "/runs; rm -rf /",
            "/runs$(id)",
            "/runs`id`",
            "relative/runs",
            "host:relative",
            "a:b:/runs"
          ] do
        assert :error = Runs.parse_target(hostile),
               "expected #{inspect(hostile)} to be refused"
      end
    end
  end
end
