defmodule IxMcp.Stdlib.ShTest do
  use ExUnit.Case, async: true

  alias IxMcp.Stdlib.Sh

  require IxMcp.Stdlib.Sh

  # Scoped to a root the caller owns. Globbing the shared /tmp made these
  # assertions flaky by construction: any concurrent run -- a neighbouring async
  # test, or the machine's real kernel in another OS process under the same
  # TMPDIR -- puts an ix-sh-* dir there between the snapshot and the assertion,
  # and "nothing leaked" then reads as a leak.
  defp scratch_dirs(root), do: Path.wildcard(Path.join(root, "ix-sh-*"))

  describe "argv discipline" do
    test "a command string is refused with the list idiom in the message" do
      error = assert_raise ArgumentError, fn -> Sh.cmd("rg -n needle lib") end
      assert error.message =~ "argv list, never a command string"
      assert error.message =~ "~w(rg -o --) ++ [pattern, path]"
    end

    test "an empty argv and non-string words are refused" do
      assert_raise ArgumentError, ~r/non-empty argv list/, fn -> Sh.cmd([]) end
      assert_raise ArgumentError, ~r/must be strings/, fn -> Sh.cmd(["echo", 42]) end
    end

    test "a word containing spaces stays ONE argument" do
      # The whole point of the list: a shell would have split this into two.
      result = Sh.run(Sh.cmd(["printf", "%s\n", "two words"]))
      assert Sh.ok?(result)
      assert result.out == "two words\n"
    end
  end

  describe "per-stage exit codes (PIPESTATUS as data)" do
    test "a failed FIRST stage is visible even though the pipeline exits 0" do
      result = Sh.pipeline([~w(false), ~w(cat), ~w(true)]) |> Sh.run()

      # What a shell would have told you: success.
      assert result.rc == 0
      # What actually happened.
      refute Sh.ok?(result)
      assert [1, 0, 0] == Enum.map(result.stages, & &1.rc)
      assert Enum.map(result.stages, & &1.argv) == [["false"], ["cat"], ["true"]]
    end

    test "every stage's rc is kept in pipeline order" do
      result = Sh.pipeline([~w(true), ~w(false)]) |> Sh.run()
      assert [0, 1] == Enum.map(result.stages, & &1.rc)
      assert result.rc == 1
      refute Sh.ok?(result)
    end

    test "ok!/1 raises with the whole stage table; ok?/1 stays a value" do
      pipeline = Sh.pipeline([~w(false), ~w(cat)])
      error = assert_raise Sh.Error, fn -> Sh.ok!(pipeline) end

      assert error.message =~ "did not succeed in every stage"
      assert error.message =~ "rc=1"
      assert error.message =~ "false"
      assert [1, 0] == Enum.map(error.result.stages, & &1.rc)
    end

    test "a missing executable is a value, not a raise" do
      result = Sh.run(Sh.cmd(["definitely-not-a-real-binary-ix"]))
      refute Sh.ok?(result)
      assert hd(result.stages).rc == 127
      assert hd(result.stages).stderr =~ "not found"
    end
  end

  describe "per-stage stderr" do
    @tag :tmp_dir
    test "each stage keeps its OWN stderr, and stdout is not polluted by it", %{tmp_dir: dir} do
      readable = Path.join(dir, "readable")
      File.write!(readable, "payload\n")

      result =
        Sh.pipeline([
          ~w(cat) ++ [readable, Path.join(dir, "missing-one")],
          ~w(cat) ++ ["-", Path.join(dir, "missing-two")]
        ])
        |> Sh.run()

      [first, second] = result.stages
      assert first.stderr =~ "missing-one"
      refute first.stderr =~ "missing-two"
      assert second.stderr =~ "missing-two"
      refute second.stderr =~ "missing-one"

      # stderr never folded into the data stream.
      assert result.out == "payload\n"
      refute Sh.ok?(result)
    end

    # The fixture is 3-byte characters against a 16384-byte cap, and
    # 16384 = 3 * 5461 + 1, so the naive tail is GUARANTEED to start on the last
    # byte of a character. The refute below is the instrument check: it proves
    # this fixture really does exercise the bug instead of merely looking like it
    # should. An invalid binary raises {:invalid_byte, _} from JSON.encode! on
    # the reply path, taking down a whole connection over a printed arrow.
    test "an oversized stderr is capped as the TAIL, on a character boundary" do
      body = "FIRST-DIAGNOSTIC" <> String.duplicate("→", 20_000) <> "LAST-DIAGNOSTIC"
      naive_tail = binary_part(body, byte_size(body) - 16_384, 16_384)
      refute String.valid?(naive_tail)

      result = Sh.run(Sh.cmd(["sh", "-c", "cat 1>&2"]), stdin: body)

      [stage] = result.stages
      assert stage.stderr_truncated
      assert stage.stderr_bytes == byte_size(body)
      assert byte_size(stage.stderr) < stage.stderr_bytes
      assert String.valid?(stage.stderr)
      assert is_binary(JSON.encode!(%{stderr: stage.stderr}))
      # The tail is kept, so the LAST diagnostic is the one that survives.
      assert stage.stderr =~ "LAST-DIAGNOSTIC"
      refute stage.stderr =~ "FIRST-DIAGNOSTIC"
      assert Sh.report(result) =~ "tail"
    end

    @tag :tmp_dir
    test "a redirect that fails at open time reports on the RIGHT stage, with a reason",
         %{tmp_dir: dir} do
      # out: names an existing DIRECTORY, so `>` fails with EISDIR at open time,
      # after the precheck (whose only concern is the parent) has passed. With
      # `2>` emitted LAST, sh's complaint went to the stderr it inherited from
      # the BEAM -- the console -- and this surfaced as a nonzero rc with EMPTY
      # stderr, while the broken pipe got blamed on the innocent upstream stage.
      sink = Path.join(dir, "sink-dir")
      File.mkdir!(sink)

      result =
        Sh.pipeline([~w(echo hello), ~w(cat)])
        |> Sh.run(out: {:file, sink}, timeout_ms: 10_000)

      refute result.timed_out, Sh.report(result)
      refute Sh.ok?(result)

      last = List.last(result.stages)
      assert last.rc != 0
      assert last.stderr_captured
      assert last.stderr =~ ~r/directory/i
    end
  end

  describe "a broken instrument is distinguishable from a true negative" do
    @tag :tmp_dir
    test "grep: bad pattern exits 2, no match exits 1", %{tmp_dir: dir} do
      haystack = Path.join(dir, "haystack")
      File.write!(haystack, "alpha\nbeta\n")

      no_match = Sh.run(Sh.cmd(~w(grep -E) ++ ["zeta", haystack]))
      broken = Sh.run(Sh.cmd(~w(grep -E) ++ ["[", haystack]))

      assert hd(no_match.stages).rc == 1, "a true negative must be rc=1"
      assert hd(broken.stages).rc == 2, "a broken pattern must be rc=2, not another empty result"

      # Both produced NO stdout: output alone cannot tell these apart, which is
      # why the rc has to be data.
      assert no_match.out == ""
      assert broken.out == ""
      assert hd(no_match.stages).stderr == ""
      assert hd(broken.stages).stderr != ""
    end

    # TAGGED, not wrapped in `if System.find_executable("rg")`: a compile-time
    # guard makes this arm VANISH where rg is absent, and a test that vanishes is
    # indistinguishable from one that was never written -- the suite count just
    # gets quietly smaller. The tag routes it through test_helper's exclude
    # contract instead, so a sandbox without rg reports it as EXCLUDED, by name.
    @tag :tmp_dir
    @tag :needs_rg
    test "rg: bad pattern exits 2, no match exits 1", %{tmp_dir: dir} do
      haystack = Path.join(dir, "haystack")
      File.write!(haystack, "alpha\nbeta\n")

      no_match = Sh.run(Sh.cmd(~w(rg --) ++ ["zeta", haystack]))
      broken = Sh.run(Sh.cmd(~w(rg --) ++ ["[", haystack]))

      assert hd(no_match.stages).rc == 1
      assert hd(broken.stages).rc == 2
    end
  end

  describe "bodies go on stdin, not in argv" do
    test "an argv word past MAX_ARG_STRLEN is refused, naming stdin as the fix" do
      body = String.duplicate("x", 131_073)
      error = assert_raise ArgumentError, fn -> Sh.cmd(["tee", body]) end

      assert error.message =~ "131072"
      assert error.message =~ "MAX_ARG_STRLEN"
      assert error.message =~ "stdin: body"
    end

    # The kernel compares strlen(arg) < MAX_ARG_STRLEN, so 131072 is the first
    # size that FAILS. An earlier version of this test asserted 131072 was
    # allowed, which certified an off-by-one that no test on darwin could ever
    # expose: the refusal is ours, the E2BIG would have been the kernel's.
    test "the limit is exclusive: 131071 is allowed, 131072 is refused" do
      assert %Sh.Step{} = Sh.cmd(["echo", String.duplicate("x", 131_071)])
      assert_raise ArgumentError, fn -> Sh.cmd(["echo", String.duplicate("x", 131_072)]) end
    end

    # execve terminates each argv string at the first NUL, so this word would
    # have reached rg as "pat" and returned a clean, confident, wrong answer.
    test "an argv word containing a NUL is refused rather than silently truncated" do
      error = assert_raise ArgumentError, fn -> Sh.cmd(["rg", "pat" <> <<0>> <> "tern"]) end

      assert error.message =~ "NUL byte at offset 3"
      assert error.message =~ "truncated prefix"
    end

    test "a 400 KB body that argv could never carry rides stdin instead" do
      # The inherited failure: a 36 KB argv body died with "Argument list too
      # long" where a 14 KB one had worked. On stdin the size stops mattering.
      body = String.duplicate("payload\n", 50_000)
      assert byte_size(body) > 131_072

      result = Sh.pipeline([~w(cat), ~w(wc -c)]) |> Sh.run(stdin: body)

      assert Sh.ok?(result), Sh.report(result)
      assert String.trim(result.out) |> String.to_integer() == byte_size(body)
    end

    @tag :tmp_dir
    test "stdin: {:file, path} feeds a file straight into stage 1", %{tmp_dir: dir} do
      source = Path.join(dir, "source.bin")
      bytes = 2 * 1024 * 1024
      File.write!(source, String.duplicate("z", bytes))

      result = Sh.pipeline([~w(cat), ~w(wc -c)]) |> Sh.run(stdin: {:file, source})

      assert Sh.ok?(result), Sh.report(result)
      assert String.trim(result.out) |> String.to_integer() == bytes
    end

    # An UNBOUNDED source is the only honest test that stdin: {:file, _} is not
    # slurped into a binary first: reading /dev/zero to EOF never returns, so an
    # eager implementation cannot pass this at all, whatever its byte counts say.
    test "an infinite stdin file proves the feed is streamed, not read to EOF first" do
      result =
        Sh.pipeline([~w(head -c 1048576), ~w(wc -c)])
        |> Sh.run(stdin: {:file, "/dev/zero"}, timeout_ms: 30_000)

      refute result.timed_out, Sh.report(result)
      assert Sh.ok?(result), Sh.report(result)
      assert String.trim(result.out) |> String.to_integer() == 1_048_576
    end

    test "a mistyped stdin path is refused before anything spawns" do
      assert_raise ArgumentError, ~r/stdin:.*not readable/s, fn ->
        Sh.run(Sh.cmd(~w(cat)), stdin: {:file, "/definitely/not/here.bin"})
      end
    end

    test "without stdin: the first stage reads EOF, so a pathless filter cannot hang" do
      # A port's stdin is a pipe the BEAM never closes; /dev/null is what keeps
      # `grep` with no file argument from waiting forever.
      result = Sh.run(Sh.cmd(~w(grep -E needle)), timeout_ms: 5_000)

      refute result.timed_out
      assert hd(result.stages).rc == 1
    end
  end

  describe "option shapes fail loudly" do
    # Falling back to a default on an unrecognised value is the silent-wrong-
    # answer shape this module exists to kill: `out: {:path, p}` used to send the
    # output to the reply instead of the file, and nothing said so.
    test "an unrecognised out:/stdin:/timeout_ms: value is refused, not defaulted" do
      assert_raise ArgumentError, ~r/out: must be/, fn ->
        Sh.run(Sh.cmd(~w(true)), out: {:path, "/tmp/nope"})
      end

      assert_raise ArgumentError, ~r/stdin: must be/, fn ->
        Sh.run(Sh.cmd(~w(cat)), stdin: :stdin)
      end

      assert_raise ArgumentError, ~r/timeout_ms: must be/, fn ->
        Sh.run(Sh.cmd(~w(true)), timeout_ms: 0)
      end

      assert_raise ArgumentError, ~r/env: entries must be/, fn ->
        Sh.run(Sh.cmd(~w(true), env: [{:PATH, "/bin"}]))
      end
    end

    test "an out: path whose parent does not exist is refused before spawning" do
      assert_raise ArgumentError, ~r|out: parent /definitely/not/here is not usable|, fn ->
        Sh.run(Sh.cmd(~w(true)), out: {:file, "/definitely/not/here/out.txt"})
      end
    end

    # :infinity used to raise ArithmeticError from the deadline arithmetic, and
    # it did so AFTER the stages were spawned, so it leaked every one of them.
    test "timeout_ms: :infinity waits rather than crashing the deadline arithmetic" do
      assert Sh.ok!(Sh.cmd(~w(printf ok)), timeout_ms: :infinity) == "ok"
      assert Sh.ok!(Sh.pipeline([~w(printf ok), ~w(cat)]), timeout_ms: :infinity) == "ok"
    end
  end

  describe "a run leaves nothing behind" do
    @tag :tmp_dir
    test "the scratch dir is removed even when the run times out", %{tmp_dir: dir} do
      result = Sh.run(Sh.cmd(~w(sleep 30)), timeout_ms: 700, scratch_root: dir)

      assert result.timed_out
      assert scratch_dirs(dir) == []
    end

    @tag :tmp_dir
    test "the scratch dir is removed when a precheck refuses the run", %{tmp_dir: dir} do
      assert_raise ArgumentError, fn ->
        Sh.run(Sh.cmd(~w(cat)), stdin: {:file, "/definitely/not/here.bin"}, scratch_root: dir)
      end

      assert scratch_dirs(dir) == []
    end

    @tag :tmp_dir
    test "scratch_root: is refused unless it is an absolute directory", %{tmp_dir: dir} do
      for bad <- ["relative/dir", Path.join(dir, "absent"), 42] do
        assert_raise ArgumentError, ~r/scratch_root:/, fn ->
          Sh.run(Sh.cmd(~w(true)), scratch_root: bad)
        end
      end
    end

    @tag :tmp_dir
    test "a timed-out stage's process tree is killed, not orphaned", %{tmp_dir: dir} do
      # `tail -f` blocks with the marker in its OWN argv, so there is no shell
      # in the middle whose exec optimisation could drop it. The search pattern
      # is bracketed (r[e]ap) so it cannot match the argv of the pgrep stage
      # that carries it: a filter that matches its own subject matter is not a
      # filter, and that is exactly how this test would fool itself.
      marker = "sh-dsl-reap-#{System.unique_integer([:positive])}"
      followed = Path.join(dir, marker)
      File.write!(followed, "")

      result = Sh.run(Sh.cmd(["tail", "-f", followed]), timeout_ms: 700)
      assert result.timed_out

      # The verdict needs pgrep, which the nix check sandbox does not ship. The
      # timeout assertion above still runs everywhere -- and it is the one that
      # caught kill_tree RAISING on a host without pgrep, turning a timeout into
      # an exception instead of a field.
      if System.find_executable("pgrep") do
        Process.sleep(300)
        pattern = String.replace(marker, "reap", "r[e]ap")
        survivors = Sh.run(Sh.cmd(["pgrep", "-f", pattern]))

        # pgrep exits 1 on no match, which is the verdict here.
        assert hd(survivors.stages).rc == 1, "orphaned: " <> survivors.out
      end
    end
  end

  describe "streaming" do
    # An unbounded producer is the only assertion that can tell streaming from
    # eager collection: `cat /dev/zero` never ends, so if any stage were
    # buffered to completion before the next started, this could not terminate
    # at all. The byte-count test below passes either way.
    test "an unbounded producer proves the stages run concurrently" do
      result =
        Sh.pipeline([~w(cat /dev/zero), ~w(head -c 1048576), ~w(wc -c)])
        |> Sh.run(timeout_ms: 30_000)

      refute result.timed_out, Sh.report(result)
      assert String.trim(result.out) |> String.to_integer() == 1_048_576
      # head exits 0; cat dies on the broken pipe. Per-stage rcs keep BOTH
      # facts, where a shell would have handed you only the last one.
      assert List.last(result.stages).rc == 0
      assert hd(result.stages).rc != 0
    end

    test "12 MiB crosses a 3-stage pipeline and only the byte count enters the VM" do
      bytes = 12 * 1024 * 1024

      result =
        Sh.pipeline([
          ~w(head -c) ++ [Integer.to_string(bytes), "/dev/zero"],
          ~w(cat),
          ~w(wc -c)
        ])
        |> Sh.run(timeout_ms: 120_000)

      assert Sh.ok?(result), Sh.report(result)
      # All 12 MiB traversed all three stages...
      assert String.trim(result.out) |> String.to_integer() == bytes
      # ...while the BEAM only ever held the byte count: intermediate stdout
      # goes stage-to-stage through an OS FIFO and never enters this VM.
      assert byte_size(result.out) < 64
      assert [0, 0, 0] == Enum.map(result.stages, & &1.rc)
    end

    @tag :tmp_dir
    test "out: {:file, path} keeps even the final stdout out of memory", %{tmp_dir: dir} do
      bytes = 4 * 1024 * 1024
      sink = Path.join(dir, "sink.bin")

      result =
        Sh.pipeline([~w(head -c) ++ [Integer.to_string(bytes), "/dev/zero"], ~w(cat)])
        |> Sh.run(out: {:file, sink}, timeout_ms: 120_000)

      assert Sh.ok?(result), Sh.report(result)
      assert result.out == ""
      assert File.stat!(sink).size == bytes
    end

    test "a bad cd: on a LATER stage raises at once instead of hanging the earlier ones" do
      # Checked per-stage inside the spawn loop, this would leave stage 1 live and
      # blocked on a FIFO nobody opens, so a typo would cost the whole timeout.
      started = System.monotonic_time(:millisecond)

      assert_raise ArgumentError, ~r|cd target /definitely/not/here is not usable|, fn ->
        Sh.pipe(Sh.cmd(~w(cat)), Sh.cmd(~w(cat), cd: "/definitely/not/here"))
        |> Sh.run(timeout_ms: 60_000)
      end

      assert System.monotonic_time(:millisecond) - started < 5_000,
             "the refusal must be immediate, not a timeout"
    end

    test "a timeout is a field, not a hang: stages that never exited carry rc: nil" do
      result = Sh.run(Sh.cmd(~w(sleep 30)), timeout_ms: 300)

      assert result.timed_out
      assert hd(result.stages).rc == nil
      refute Sh.ok?(result)
      assert Sh.report(result) =~ "TIMED OUT"
      # Was "never exited", which was a lie about the common case: the stage DID
      # exit, we SIGKILLed it on timeout and discarded the status by closing the
      # port. The polarity of this assertion flips rather than disappearing, so
      # the history records that the wording was once wrong.
      assert Sh.report(result) =~ "killed on timeout"
    end
  end

  describe "mutate/verify" do
    @tag :tmp_dir
    test "a mutation whose own output claims success but changed nothing FAILS", %{tmp_dir: dir} do
      marker = Path.join(dir, "marker")

      error =
        assert_raise Sh.VerifyError, fn ->
          Sh.mutate "create the marker" do
            # Its own stdout says it worked. The world disagrees.
            Sh.cmd(~w(echo created the marker)) |> Sh.run()
            verify
            File.exists?(marker)
          end
        end

      assert error.label == "create the marker"
      assert error.message =~ "postconditions do not hold"
      assert error.message =~ "File.exists?(marker)"
      refute File.exists?(marker)
    end

    @tag :tmp_dir
    test "verify clauses read the world FRESH, after the mutation", %{tmp_dir: dir} do
      marker = Path.join(dir, "marker")

      value =
        Sh.mutate "create the marker" do
          Sh.cmd(~w(touch) ++ [marker]) |> Sh.run()
          verify
          File.exists?(marker)
          Sh.ok!(Sh.cmd(~w(wc -c) ++ [marker])) =~ "0"
        end

      assert %Sh.Result{} = value
      assert Sh.ok?(value)
      assert File.exists?(marker)
    end

    @tag :tmp_dir
    test "a failed comparison reports BOTH sides' values", %{tmp_dir: dir} do
      path = Path.join(dir, "content")

      error =
        assert_raise Sh.VerifyError, fn ->
          Sh.mutate "write the expected content" do
            File.write!(path, "actual\n")
            verify
            String.trim(File.read!(path)) == "expected"
          end
        end

      assert error.message =~ ~s(left:  "actual")
      assert error.message =~ ~s(right: "expected")
    end

    @tag :tmp_dir
    test "a verify clause that RAISES counts as a failure, not a pass", %{tmp_dir: dir} do
      error =
        assert_raise Sh.VerifyError, fn ->
          Sh.mutate "touch a marker" do
            File.write!(Path.join(dir, "marker"), "x")
            verify
            Sh.ok!(Sh.cmd(~w(cat) ++ [Path.join(dir, "absent")])) == ""
          end
        end

      assert error.message =~ "raised:"
      assert error.message =~ "did not succeed in every stage"
    end

    test "a mutate with no verify section is refused at compile time" do
      assert_raise ArgumentError, ~r/needs a `verify` section/, fn ->
        Code.eval_string("""
        require IxMcp.Stdlib.Sh
        IxMcp.Stdlib.Sh.mutate "no postcondition" do
          :ok
        end
        """)
      end
    end
  end

  describe "watch: an instrument must be able to say both yes and no" do
    test "refuses a pattern that has not been shown to match anything" do
      assert {:error, reason} =
               Sh.arm("gate verdict",
                 pattern: ~r/^gate: (PASS|FAIL)/m,
                 must_match: "the gate said nothing of the sort",
                 must_not_match: "irrelevant"
               )

      assert reason =~ "does not match its positive control"
      assert reason =~ "never shown to say YES"
    end

    test "refuses a vocabulary pattern that matches its own negative fixture" do
      # The real 2026-08-11 miss: `REFUS` matched the NAME of a passing arm.
      assert {:error, reason} =
               Sh.arm("landing monitor",
                 pattern: ~r/REFUS/,
                 must_match: "gate: REFUSED",
                 must_not_match: "an UNDECLARED refusing instrument REFUSES (rc=1)"
               )

      assert reason =~ "ALSO matches its negative control"
      assert reason =~ "anchor to the runner's verdict line"
    end

    test "arms a verdict-anchored pattern and matches only the verdict" do
      assert {:ok, watch} =
               Sh.arm("landing monitor",
                 pattern: ~r/^gate: REFUSED/m,
                 must_match: "checks done\ngate: REFUSED\n",
                 must_not_match: "an UNDECLARED refusing instrument REFUSES (rc=1)"
               )

      assert Sh.matches?(watch, "gate: REFUSED")
      refute Sh.matches?(watch, "an UNDECLARED refusing instrument REFUSES (rc=1)")
    end

    test "the macro refuses a blind watcher at COMPILE time" do
      # The watcher sits inside a fn that is NEVER INVOKED, so only the expansion
      # can raise. Written as a bare expression, this test stayed green with the
      # whole compile-time gate deleted -- watch/2's expansion raises at run time
      # too, and Code.eval_string both compiles AND evaluates, so it could not
      # tell the two apart. That distinction is the entire claim it defends.
      error =
        assert_raise ArgumentError, fn ->
          Code.eval_string("""
          require IxMcp.Stdlib.Sh
          fn ->
            IxMcp.Stdlib.Sh.watch("blind",
              pattern: ~r/REFUS/,
              must_match: "gate: REFUSED",
              must_not_match: "an UNDECLARED refusing instrument REFUSES (rc=1)")
          end
          """)
        end

      assert error.message =~ "ALSO matches its negative control"
    end

    test "a MISSING control fails the build, not the incident" do
      # literal_binary(nil) returned :error, the compile-time `with` read that as
      # "not a literal, cannot judge here", and so the blindest watcher there is --
      # one carrying NO negative control at all -- compiled clean and then died as
      # a KeyError at arm time. Absence in a literal opts list is decidable at
      # compile time, which makes it the one case that must not be waved through.
      error =
        assert_raise ArgumentError, fn ->
          Code.eval_string("""
          require IxMcp.Stdlib.Sh
          fn ->
            IxMcp.Stdlib.Sh.watch("blind", pattern: ~r/FAIL/, must_match: "gate: FAIL 3")
          end
          """)
        end

      assert error.message =~ "declares no :must_not_match"
    end

    test "arm/2 reports a missing control as data, never as a KeyError" do
      # The runtime half of the same hole: Keyword.fetch! raised KeyError, which is
      # not the {:error, reason} shape every caller of arm/2 branches on.
      assert {:error, reason} = Sh.arm("blind", pattern: ~r/FAIL/, must_match: "gate: FAIL 3")
      assert reason =~ "a control is MISSING"

      assert {:error, _reason} = Sh.arm("blind", pattern: ~r/FAIL/, must_not_match: "passing arm")
    end

    test "the macro returns the watch when both controls pass" do
      watch =
        Sh.watch("gate verdict",
          pattern: ~r/^gate: (PASS|FAIL)/m,
          must_match: "gate: FAIL 3 checks",
          must_not_match: "an UNDECLARED refusing instrument REFUSES (rc=1)"
        )

      assert %Sh.Watch{label: "gate verdict"} = watch
    end

    test "a runtime-assembled pattern gets the same two controls" do
      assert {:ok, _watch} =
               Sh.arm("dynamic", pattern: "^done", must_match: "done", must_not_match: "undone")

      # `must_match: "done"` against pattern "done" is refused by the BYTE-EQUAL
      # branch, which returns first, so this test never reached the negative
      # control at all and stayed green with that branch deleted. A positive
      # control that is not byte-equal is what makes the negative one reachable.
      assert {:error, reason} =
               Sh.arm("dynamic",
                 pattern: "done",
                 must_match: "all done",
                 must_not_match: "done pending"
               )

      assert reason =~ "ALSO matches its negative control"
    end
  end

  describe "until/3" do
    test "matches as soon as the verdict appears" do
      {:ok, watch} =
        Sh.arm("verdict", pattern: ~r/^PASS/m, must_match: "PASS", must_not_match: "passing arm")

      counter = :counters.new(1, [])

      probe = fn ->
        :counters.add(counter, 1, 1)
        if :counters.get(counter, 1) >= 3, do: "PASS\n", else: "still working\n"
      end

      assert {:matched, ["PASS"]} = Sh.until(watch, probe, interval_ms: 1, timeout_ms: 5_000)
    end

    test "done? ends the wait instead of guessing that silence is a stall" do
      {:ok, watch} =
        Sh.arm("verdict", pattern: ~r/^PASS/m, must_match: "PASS", must_not_match: "passing arm")

      assert {:ended, "no verdict\n"} =
               Sh.until(watch, fn -> "no verdict\n" end,
                 interval_ms: 1,
                 timeout_ms: 60_000,
                 done?: fn -> true end
               )
    end

    test "a verdict printed in the same breath as finishing still reads as matched" do
      {:ok, watch} =
        Sh.arm("verdict", pattern: ~r/^PASS/m, must_match: "PASS", must_not_match: "passing arm")

      assert {:matched, _captures} =
               Sh.until(watch, fn -> "PASS\n" end,
                 interval_ms: 1,
                 timeout_ms: 5_000,
                 done?: fn -> true end
               )
    end

    test "a verdict printed BETWEEN the probe and the terminal check reads as matched" do
      {:ok, watch} =
        Sh.arm("verdict", pattern: ~r/^PASS/m, must_match: "PASS", must_not_match: "passing arm")

      # The DISCRIMINATING case, which the test above cannot see: its probe returns
      # "PASS\n" on every call, so match-first is enough. Here the first read is
      # silent and the verdict exists only on the second, while done? already says
      # the work finished. The text was read at T0 and done? answers about T1, so
      # this came back {:ended, "still working\n"} -- "finished without a verdict"
      # -- about a run that had in fact passed. Ordering the checks within one
      # iteration does not close a window that spans two instants.
      counter = :counters.new(1, [])

      probe = fn ->
        :counters.add(counter, 1, 1)
        if :counters.get(counter, 1) >= 2, do: "PASS\n", else: "still working\n"
      end

      assert {:matched, ["PASS"]} =
               Sh.until(watch, probe, interval_ms: 1, timeout_ms: 5_000, done?: fn -> true end)
    end
  end

  describe "a timeout is a FIELD, never an exception" do
    @tag :tmp_dir
    test "200 concurrent timeouts all come back as data", %{tmp_dir: dir} do
      # `if Port.info(port) != nil, do: Port.close(port)` was CHECK-THEN-ACT: the
      # SIGKILL sent one line earlier reaped the port inside the window, and
      # port_close/1 RAISES on an already-closed port. Any raise escapes its task
      # and fails this test.
      #
      # This detector is PROBABILISTIC and the count is chosen deliberately. The
      # race was measured at roughly 1.4% per timed-out port (2 of 144, then 1-2
      # of 192), so 24 ports would catch a regression under a third of the time --
      # verified useless by reverting the fix, which left the suite GREEN. At 200
      # ports the odds of missing it are about 6%. The sequential double-close
      # cannot be tested instead: with the old guard in place it takes the
      # skip-the-close branch and passes, so only real contention separates them.
      count = 200

      results =
        1..count
        |> Task.async_stream(
          fn i ->
            Sh.run(Sh.pipeline([~w(sleep 30), ~w(cat)]),
              timeout_ms: 100 + rem(i, 50),
              scratch_root: dir
            )
          end,
          max_concurrency: 40,
          timeout: 120_000
        )
        |> Enum.map(fn {:ok, result} -> result end)

      assert length(results) == count
      assert Enum.all?(results, & &1.timed_out)
      assert Enum.all?(results, &(&1.rc == nil))
      assert scratch_dirs(dir) == []
    end

    test "closing is attempted unconditionally, never asked about first" do
      # The deterministic half of C1. The fix is not "cope with a closed port",
      # it is "do not ASK first", because the question and the answer cannot be
      # made atomic. A term that is not a port at all separates the two shapes
      # without needing the race: Port.info/1 raises on it just as Port.close/1
      # does, so only the version that wraps the CLOSE survives.
      #
      # It must not be an ATOM: `Port.info(:not_a_port)` treats the atom as a
      # REGISTERED PORT NAME and returns nil instead of raising, which made the
      # first version of this test pass against the reverted fix. Verified with a
      # probe: 42, "str", {1, 2} and a pid all raise from both calls.
      assert :ok = Sh.close_port(42)

      port =
        Port.open({:spawn_executable, "/bin/sh"}, [:binary, :exit_status, args: ["-c", "exit 0"]])

      assert :ok = Sh.close_port(port)
      assert :ok = Sh.close_port(port)
    end
  end

  describe "the collector touches only the ports this run owns" do
    @tag :tmp_dir
    test "a foreign port's payload AND its exit status survive a run", %{tmp_dir: dir} do
      # Unbound `_port` clauses drained every port the cell process owned, so a
      # neighbouring port's data and exit status both vanished with
      # message_queue_len back at 0 and nothing saying anything was eaten.
      foreign =
        Port.open({:spawn_executable, "/bin/sh"}, [
          :binary,
          :exit_status,
          args: ["-c", "sleep 0.3; printf FOREIGN-PAYLOAD"]
        ])

      assert Sh.ok?(Sh.run(Sh.cmd(~w(sleep 1)), scratch_root: dir))

      assert_receive {^foreign, {:data, "FOREIGN-PAYLOAD"}}, 3_000
      assert_receive {^foreign, {:exit_status, 0}}, 3_000
    end
  end

  describe "refusals that a public struct cannot walk around" do
    test "a hand-built Step cannot smuggle a NUL past cmd/2" do
      # Validation used to live only in cmd/2, and `%Sh.Step{}` is public: this
      # ran as `echo pat` and returned rc=0 with "pat\n" -- the clean, confident,
      # wrong answer the guard exists to prevent.
      step = %Sh.Step{argv: ["echo", "pat" <> <<0>> <> "tern"]}
      assert_raise ArgumentError, ~r/NUL/, fn -> Sh.run(step) end
    end

    @tag :tmp_dir
    test "a relative out: or stdin: path is refused, not validated in the wrong dir", %{
      tmp_dir: dir
    } do
      # `Path.dirname("out.txt") == "."`, so this validated the package dir and
      # then wrote to the stage's cd: -- one directory checked, another written.
      assert_raise ArgumentError, ~r/must be an absolute path/, fn ->
        Sh.run(Sh.cmd(~w(printf hi), cd: dir), out: {:file, "sh-relative-out.txt"})
      end

      assert_raise ArgumentError, ~r/must be an absolute path/, fn ->
        Sh.run(Sh.cmd(~w(cat), cd: dir), stdin: {:file, "sh-relative-in.txt"})
      end
    end

    test "a NUL in an io path is refused before it can kill the shell's parser" do
      # A NUL truncates the script mid-quote, so sh dies in its PARSER before any
      # redirect -- including the stage's own 2>, which is why the real
      # diagnostic went to the BEAM console and came back rc=2, uncaptured.
      assert_raise ArgumentError, ~r/NUL/, fn ->
        Sh.run(Sh.cmd(~w(true)), out: {:file, "/tmp/sh-nul" <> <<0>> <> ".txt"})
      end
    end

    @tag :tmp_dir
    test "env entries are refused for NUL and for `=` in a name", %{tmp_dir: dir} do
      # All three passed the shape check and then raised ArgumentError from deep
      # inside Port.open, mid-spawn, as an opaque "invalid option in list".
      for env <- [[{"IX_T", "a" <> <<0>> <> "b"}], [{"IX" <> <<0>>, "1"}], [{"A=1 B", "2"}]] do
        assert_raise ArgumentError, ~r/env:/, fn ->
          Sh.run(Sh.cmd(~w(printf x), env: env), scratch_root: dir)
        end
      end
    end

    @tag :tmp_dir
    test "a stdin list that is not iodata is a named refusal", %{tmp_dir: dir} do
      assert_raise ArgumentError, ~r/stdin: list must be iodata/, fn ->
        Sh.run(Sh.cmd(~w(cat)), stdin: [:not, "iodata"], scratch_root: dir)
      end
    end
  end

  describe "mutate: a mutation that RAISES still reads the world" do
    @tag :tmp_dir
    test "the postconditions are checked and reported when the mutation raises", %{tmp_dir: dir} do
      # The moduledoc's own motivating scenario: a submit that comes back
      # "refused" may already have landed. This used to skip verification
      # entirely, so the one case that most needs a fresh read never got one.
      marker = Path.join(dir, "landed")

      error =
        assert_raise Sh.VerifyError, fn ->
          Sh.mutate "submit" do
            File.touch!(marker)
            raise "submit refused"
            verify
            File.exists?(marker)
          end
        end

      assert error.message =~ "RAISED"
      assert error.message =~ "postconditions all HOLD"
      assert error.message =~ "do not blindly retry"
      assert error.failures == []
      assert %RuntimeError{} = error.mutation_error
      assert File.exists?(marker)
    end

    @tag :tmp_dir
    test "a mutation that raises with nothing moved says that instead", %{tmp_dir: dir} do
      absent = Path.join(dir, "absent")

      error =
        assert_raise Sh.VerifyError, fn ->
          Sh.mutate "submit" do
            raise "submit refused"
            verify
            File.exists?(absent)
          end
        end

      assert error.message =~ "RAISED"
      assert error.message =~ "postconditions do NOT hold"
      refute error.failures == []
    end
  end

  describe "mutate refuses evidence that is not a fresh read" do
    test "a verify clause reusing a mutation binding is refused at compile time" do
      # Doctrine made mechanical: `r = Sh.run(...)` then `verify Sh.ok?(r)` PASSED
      # with nothing mutated, which is the exact shape the docstring forbids.
      error =
        assert_raise ArgumentError, fn ->
          Code.eval_string("""
          require IxMcp.Stdlib.Sh
          IxMcp.Stdlib.Sh.mutate "create" do
            r = IxMcp.Stdlib.Sh.run(IxMcp.Stdlib.Sh.cmd(~w(echo created)))
            verify
            IxMcp.Stdlib.Sh.ok?(r)
          end
          """)
        end

      assert error.message =~ "the verify half reads `r`"
      assert error.message =~ "go ask the world again"
    end

    test "a mutation half with no call at all is refused at compile time" do
      # The old "empty mutation half" refusal was cosmetic: a literal `:ok`
      # satisfied it, and one of this module's own tests did exactly that.
      error =
        assert_raise ArgumentError, fn ->
          Code.eval_string("""
          require IxMcp.Stdlib.Sh
          IxMcp.Stdlib.Sh.mutate "nothing" do
            :ok
            verify
            1 == 1
          end
          """)
        end

      assert error.message =~ "contains no call"
    end
  end

  describe "a watcher cannot arm blind" do
    test "a blank control is refused" do
      # The verbatim 2026-08-11 vocabulary pattern, armed through the front door:
      # every regex fails to match "", so an empty negative control is a rubber
      # stamp rather than a test.
      assert {:error, reason} =
               Sh.arm("landing monitor",
                 pattern: ~r/REFUS/,
                 must_match: "REFUS",
                 must_not_match: ""
               )

      assert reason =~ "may not be blank"
      assert reason =~ "rubber stamp"
    end

    test "a whitespace-only control is refused too" do
      assert {:error, reason} =
               Sh.arm("landing monitor",
                 pattern: ~r/REFUS/,
                 must_match: "   ",
                 must_not_match: "quiet"
               )

      assert reason =~ "may not be blank"
    end

    test "a positive control byte-equal to the pattern source is refused" do
      assert {:error, reason} =
               Sh.arm("gate verdict",
                 pattern: ~r/gate: FAIL/,
                 must_match: "gate: FAIL",
                 must_not_match: "gate: PASS"
               )

      assert reason =~ "byte-equal to the pattern source"
      assert reason =~ "a regex matches itself"
    end

    test "the compile-time macro inherits the same refusal" do
      assert_raise ArgumentError, ~r/may not be blank/, fn ->
        Code.eval_string("""
        require IxMcp.Stdlib.Sh
        IxMcp.Stdlib.Sh.watch("blind",
          pattern: ~r/REFUS/,
          must_match: "REFUS",
          must_not_match: ""
        )
        """)
      end
    end
  end

  describe "until/3 waits without guessing" do
    test "timeout_ms: :infinity does not crash the deadline arithmetic" do
      # The identical defect that collect/2 was already fixed for, one function
      # away: `now + :infinity` is an ArithmeticError.
      watcher =
        Sh.watch("verdict", pattern: ~r/^done$/m, must_match: "done", must_not_match: "pending")

      assert {:matched, _captures} =
               Sh.until(watcher, fn -> "done" end, timeout_ms: :infinity, interval_ms: 5)
    end

    test "a zero interval and a negative timeout are refused" do
      watcher =
        Sh.watch("verdict", pattern: ~r/^done$/m, must_match: "done", must_not_match: "pending")

      assert_raise ArgumentError, ~r/interval_ms:/, fn ->
        Sh.until(watcher, fn -> "" end, interval_ms: 0)
      end

      assert_raise ArgumentError, ~r/timeout_ms:/, fn ->
        Sh.until(watcher, fn -> "" end, timeout_ms: -1)
      end
    end
  end

  describe "an unknown option is a refusal, not a default" do
    test "a misspelt run option is refused instead of silently defaulted" do
      # `timout_ms: 5` kept the 600_000 ms default and said nothing, so a caller
      # who believed they had set a 5 ms timeout got a confident wrong answer ten
      # minutes later. This module's own cardinal sin wearing a keyword list.
      error =
        assert_raise ArgumentError, fn -> Sh.run(Sh.cmd(~w(echo hi)), timout_ms: 5) end

      assert error.message =~ "unknown option"
      assert error.message =~ "timout_ms"
    end

    test "the known options are all still accepted", %{} do
      # The negative control: a refusal that also refuses correct calls is worse
      # than no refusal, so every key the module reads is exercised here.
      assert %Sh.Result{} =
               Sh.run(Sh.cmd(~w(cat)),
                 stdin: "x",
                 timeout_ms: 5_000,
                 scratch_root: System.tmp_dir!()
               )
    end
  end

  describe "the stderr cap tells the truth" do
    @tag :tmp_dir
    test "a stderr that only exceeds the cap after sanitizing still says truncated", %{
      tmp_dir: dir
    } do
      # 5000 invalid bytes sit comfortably under the 16 KiB cap as bytes on disk,
      # but each becomes a four-character escape, so the sanitized text is ~20 KB
      # and loses its head on the way out. Deriving the flag from the RAW size
      # reported stderr_truncated: false about text that had been cut -- a capped
      # stderr mistaken for a short one, the one thing this cap promises not to do.
      result =
        Sh.run(Sh.cmd(["sh", "-c", "head -c 5000 /dev/zero | tr '\\0' '\\377' 1>&2"]),
          scratch_root: dir
        )

      stage = hd(result.stages)

      assert stage.stderr_bytes == 5000
      assert byte_size(stage.stderr) <= 16_384
      assert stage.stderr_truncated
    end

    @tag :tmp_dir
    test "stderr that was never valid UTF-8 comes back JSON-encodable", %{tmp_dir: dir} do
      # Nothing in the suite emitted invalid UTF-8, so the sanitize leg of the
      # cap was untested: the old fixture's tail became valid after the
      # continuation-byte trim alone.
      result = Sh.run(Sh.cmd(["sh", "-c", "printf '\\377\\376bad\\n' 1>&2"]), scratch_root: dir)
      [stage] = result.stages

      assert String.valid?(stage.stderr)
      assert stage.stderr =~ "bad"
      assert stage.stderr =~ "\\xFF"
    end

    @tag :tmp_dir
    test "escaping cannot push the kept stderr past the cap", %{tmp_dir: dir} do
      # Capping BEFORE sanitizing let each invalid byte become four characters
      # afterwards, so a 16 KiB tail came back as 37,579 bytes while report/1
      # still announced "tail 16384B".
      result =
        Sh.run(Sh.cmd(["sh", "-c", "head -c 204800 /dev/urandom 1>&2"]),
          timeout_ms: 60_000,
          scratch_root: dir
        )

      [stage] = result.stages

      assert stage.stderr_bytes == 204_800
      assert stage.stderr_truncated
      assert byte_size(stage.stderr) <= 16_384
      assert String.valid?(stage.stderr)
    end
  end

  describe "the redirect order buys EOF instead of deadlock" do
    @tag :tmp_dir
    test "a stdin file that stats but cannot be opened ends the pipeline fast", %{tmp_dir: dir} do
      # The untested half of the redirect fix. File.stat succeeds, so the
      # precheck passes; the FIFOs are opened before the caller's stdin, so
      # stage 0's failure is an EOF for the rest instead of a blocked open.
      source = Path.join(dir, "unreadable.bin")
      File.write!(source, "payload")
      File.chmod!(source, 0o000)

      result =
        Sh.run(Sh.pipeline([~w(cat), ~w(cat), ~w(wc -c)]),
          stdin: {:file, source},
          timeout_ms: 5_000,
          scratch_root: dir
        )

      refute result.timed_out

      # root ignores the mode bits, which would make the premise false; the
      # no-deadlock assertion above holds either way.
      if match?({:error, :eacces}, File.read(source)) do
        [first | _rest] = result.stages
        assert first.rc == 1
        assert first.stderr =~ "Permission denied"
      end
    end
  end

  describe "the descendant sweep is best effort, and that is load-bearing" do
    test "a sweeper that raises still kills the stage" do
      # This is the entire basis of "a timeout is a field, not an exception": on
      # a host WITH pgrep the fallback is unreachable, and on the host where it
      # runs (the nix sandbox) the verifying assertion is the one that is
      # skipped. So the contract is exercised directly.
      port =
        Port.open({:spawn_executable, "/bin/sh"}, [
          :binary,
          :exit_status,
          args: ["-c", "exec sleep 30"]
        ])

      {:os_pid, os_pid} = Port.info(port, :os_pid)

      assert :ok =
               Sh.kill_tree_or_signal(os_pid, fn _pid ->
                 raise "pgrep: no such file or directory"
               end)

      assert_receive {^port, {:exit_status, _status}}, 5_000
    end
  end

  describe "ok!/2 forwards its options" do
    @tag :tmp_dir
    test "a timeout passed to ok!/2 really takes effect", %{tmp_dir: dir} do
      # The old test asserted `ok!(cmd, timeout_ms: :infinity) == "ok"`, which is
      # equally true of an ok!/2 that DISCARDS its options. This one is false
      # unless the option arrives: without it the sleep finishes and returns "".
      error =
        assert_raise Sh.Error, fn ->
          Sh.ok!(Sh.cmd(~w(sleep 5)), timeout_ms: 200, scratch_root: dir)
        end

      assert error.result.timed_out
    end
  end
end
