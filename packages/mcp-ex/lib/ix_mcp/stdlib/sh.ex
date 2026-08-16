defmodule IxMcp.Stdlib.Sh do
  @moduledoc """
  OS pipelines as data: argv lists in, per-stage exit codes and stderr out.

  A shell pipeline is the least observable thing an agent can run. `a | b | c`
  reports one exit status out of three, throws away every stage's stderr
  unless someone remembered `2>&1`, and re-splits every interpolated variable
  on whitespace. Each of those has cost real debugging hours here, so `Sh`
  takes the shell out of the loop: each stage is spawned from its own argv
  list with no shell word-splitting anywhere, each stage's `rc` and `stderr`
  come back as fields you can pattern-match, and `run/2` never raises on a
  command failure -- the failure is a value you inspect, not an exception you
  remember to catch.

  ## The traps this exists to kill

    * **PIPESTATUS invisibility.** `a | b` exits with `b`'s status, so a
      failed `a` reads as success; every stage's rc is in `result.stages`.
    * **The `grep -qv` shim lie.** An interactive shell here resolves `grep`
      to a ugrep shim under which `grep -v PAT` exits 0 while `grep -qv PAT`
      exits 1 on the same input, so every guard shaped
      `... | grep -qv X && fail` silently never fires. `Sh` spawns the real
      binary by argv with no shell and no function shim in the path.
    * **rc-1 tail traps.** `rg -c` prints nothing and exits 1 when there are
      no matches rather than printing `0`, so comparing its output to `"0"`
      is false in exactly the case being tested for. The rc carries the
      answer and `Sh` keeps it.
    * **Vocabulary-matching watchers.** A filter built from failure words
      (FAIL, REFUS, error) matches the *names of passing test arms*, because
      a good arm is named after what it proves gets refused. `watch/2`
      refuses to arm a pattern that has not been shown to match a positive
      fixture and to reject a negative one.
    * **Silent word re-splitting.** `"rg #{"#"}{pat}"` breaks the moment `pat`
      holds a space. `cmd/2` takes a list, and the idiom for variables is to
      append them as elements -- interpolating inside `~w` re-splits on
      whitespace and defeats the point:

          Sh.cmd(~w(rg -o --) ++ [pattern, path])

    * **Argv is for arguments, never for bodies.** Linux caps a SINGLE argv
      string at MAX_ARG_STRLEN = 131072 bytes however large an ARG_MAX the host
      advertises; the kernel test is `strlen(arg) < MAX_ARG_STRLEN`, so 131071
      bytes is the largest word that fits and 131072 already fails. Over it the
      spawn dies with a bare "Argument list too long" -- a 14 KB body goes
      through where a 36 KB one does not, and base64 multiplies a body by 1.78
      on the way in. `cmd/2` refuses an oversized word up front and names the
      fix: put the body on `stdin:`. Note what that guard does NOT cover: the
      per-word limit is not a TOTAL, and the budget for argv as a whole is
      `max(min(6MiB, RLIMIT_STACK / 4), ARG_MAX)` -- host-dependent, so no fixed
      number here could refuse correctly without also refusing callers that
      work. A list of thousands of paths therefore belongs on `stdin:` behind
      `xargs`, not spread across argv.

    * **An ambiguous error is not evidence the action did not happen.** A
      mutation that comes back `{:error, "submit refused"}` may already have
      succeeded: a forge submit that reached the queue surfaces as a refusal
      when the reply is lost. NEVER retry a mutation on an ambiguous error --
      read the world first (`jj --ignore-working-copy op log -n 5` names a real
      `submit <change12> to main`), because a blind retry is how one submit
      becomes two, and a double-submit is worse than a slow one. This is
      `mutate/verify` in miniature: the verdict lives in a fresh read, never in
      the mutation's own reply.

  ## Pipelines

  Stages are joined by real OS pipes (FIFOs), so a middle stage's output
  never enters the BEAM and never lands in one big binary -- backpressure is
  the kernel's pipe buffer, exactly as in a shell. Only the LAST stage's
  stdout is collected, and you choose the last stage:

      Sh.cmd(~w(rg -o --) ++ [pattern, path])
      |> Sh.pipe(Sh.cmd(~w(sort)))
      |> Sh.pipe(Sh.cmd(~w(uniq -c)))
      |> Sh.run()

  `pipeline/1` is the same thing for a list of argv lists:

      Sh.pipeline([~w(rg -n TODO .), ~w(cut -d: -f1), ~w(uniq -c)]) |> Sh.run()

  A `Result` holds one `Stage` per command, in order:

      %Sh.Result{
        out: "  3 lib/a.ex\\n",
        rc: 0,
        stages: [
          %Sh.Stage{argv: ["rg", "-n", "TODO", "."], rc: 0, stderr: "", duration_ms: 12},
          ...
        ]
      }

  `run/2` returns that struct whatever happened. `ok!/1` is the unwrapping
  form: stdout when EVERY stage exited 0, otherwise a raise carrying the
  whole stage table, because "the last stage was fine" is not the same claim
  as "the pipeline worked".

  ## Mutations verify against fresh reads

  A mutation's own output can claim success while the world did not change,
  so `mutate/2` re-reads the world after the fact. Clauses after `verify`
  run only once the mutation has finished, and they must issue their own
  commands or file reads -- never reuse the mutation's captured output:

      Sh.mutate "advance the bookmark" do
        Sh.cmd(~w(jj bookmark set main -r @)) |> Sh.run()
      verify
        Sh.ok!(Sh.cmd(~w(jj log -r main --no-graph -T commit_id))) == expected_id
        File.exists?(marker)
      end

  A failed clause raises `VerifyError` naming the clause's source text and,
  for a comparison, the value of each side.

  ## Watchers prove they can say both yes and no

      {:ok, w} = Sh.arm("gate verdict",
        pattern: ~r/^gate: (PASS|FAIL)/m,
        must_match: "gate: FAIL 3 checks",
        must_not_match: "an UNDECLARED refusing instrument REFUSES (rc=1)")

  Anchor patterns to the runner's own verdict line (`^FAIL`,
  `"status": "passed"`, `build_rc=`), never to vocabulary that can appear in
  a subject line. `watch/2` is the macro form and validates literal patterns
  at COMPILE time, so a blind watcher fails the build rather than the
  incident.

  A pattern that can match its own subject matter is not a filter, and the
  purest form of that bug is a watcher whose pattern matches its OWN argv. A
  specimen observed running on a build host while this module was being written:
  `while pgrep -f "nix store gc"; do sleep 20; done`, written to wait out a
  garbage collection. `pgrep -f` matches whole command lines, so the loop's own
  shell satisfied the pattern on every iteration and the wait could never end --
  silently, and indistinguishable from a GC that genuinely never finished.
  `must_not_match` is the arm that refuses exactly this: hand it a line the
  watcher must REJECT, here the loop's own command line, and a self-matching
  pattern dies at arm time instead of hanging for hours.

  ## Host dependencies

  `/bin/sh` for every stage, and `mkfifo` for any pipeline of more than one
  stage: the FIFOs joining the stages are what keep intermediate bytes out of
  the BEAM. Both are assumed present, not probed.

  ## Two clocks, on purpose

  `result.duration_ms` is measured from before the precheck, while `timeout_ms`
  governs only the collection phase, which starts once every stage is spawned.
  So `duration_ms > timeout_ms` is normal on a timed-out run and is not a sign
  the timeout was late: the difference is validation plus spawn.

  ## Roadmap (deliberately not in v1)

  Remote-durable blocks -- shipping a pipeline to a fleet node and surviving
  a disconnect -- will ride `IxMcp.Jobs` once the BEAM mesh is up. v1 is
  local pipelines, `mutate/2`, and watcher arming.

  ## Provenance

  2026-08-13. Two failure modes of shell-shaped tooling, both measured on this
  fleet rather than theorised. First, an interactive shell here resolves `grep`
  to a wrapper under which `grep -v PAT` exits 0 while `grep -qv PAT` exits 1
  on identical input, so every guard shaped `... | grep -qv X && fail` never
  fires and the scan it protects reports a clean zero. Second, a pipeline
  carries only its last stage's exit status, so a gate whose first stage died
  on a malformed pattern certifies its input as clean instead of refusing to
  answer. Neither is a mistake a more careful author avoids; both are
  properties of a surface that hides per-stage rc and stderr, which is why the
  fix belongs in the surface and not in the discipline of every caller.
  """

  alias IxMcp.Cmd
  alias IxMcp.GitGuard
  alias IxMcp.OsProc
  alias IxMcp.UTF8

  require Logger

  # stderr is capped per stage, keeping the TAIL: a stage that emits pages of
  # warnings before the real diagnostic puts the useful line last. The
  # original byte count and a truncation flag ride along, so a capped stderr
  # is never mistaken for a short one.
  @stderr_cap 16_384
  # Linux caps ONE argv string at MAX_ARG_STRLEN, a kernel constant of 32 pages
  # that the much larger ARG_MAX a host advertises does not raise. Over it the
  # spawn dies with a bare "Argument list too long", so the check belongs at
  # cmd/2 where the fix (stdin:) can be named. The kernel compares with a strict
  # `strlen(arg) < MAX_ARG_STRLEN`, so this value is the first REFUSED size, not
  # the last accepted one -- hence `>=` at the guard.
  @max_arg_strlen 131_072
  @default_timeout_ms 600_000
  @shell "/bin/sh"

  # Shells whose `-c` argument is a script the git guard must read rather than
  # treat as an opaque operand. `nu` is this fleet's interactive shell and `fish`
  # is common enough to matter; both spell the script flag `-c`.
  @shells ~w(sh bash zsh dash ksh nu fish)

  # Programs that exec another program, so a guard reading only argv[0] sees the
  # wrapper and never the git command behind it. Measured bypasses: `env git add
  # -A`, `nice git push`, `busybox sh -c "git add -A"`.
  @wrappers ~w(env nice ionice nohup setsid stdbuf time timeout xargs busybox sudo doas)

  defmodule Step do
    @moduledoc "One OS command: an argv list plus spawn options. Built by `IxMcp.Stdlib.Sh.cmd/2`."
    @type t :: %__MODULE__{argv: [binary(), ...], opts: keyword()}
    @enforce_keys [:argv]
    defstruct argv: nil, opts: []
  end

  defmodule Pipeline do
    @moduledoc "Steps whose stdout feeds the next stage's stdin. Built by `IxMcp.Stdlib.Sh.pipe/2`."
    @type t :: %__MODULE__{steps: [Step.t(), ...]}
    @enforce_keys [:steps]
    defstruct steps: []
  end

  defmodule Stage do
    @moduledoc """
    What one stage of a finished pipeline did.

    `rc` is `nil` only when the run hit its timeout before this stage exited.
    `duration_ms` is spawn-to-exit for that stage; stages run concurrently,
    so these overlap rather than summing to the run's wall time.
    """
    @type t :: %__MODULE__{
            argv: [binary(), ...],
            rc: non_neg_integer() | nil,
            stderr: binary(),
            stderr_bytes: non_neg_integer(),
            stderr_truncated: boolean(),
            stderr_captured: boolean(),
            duration_ms: non_neg_integer()
          }
    defstruct [
      :argv,
      :rc,
      :stderr,
      :stderr_bytes,
      :stderr_truncated,
      :duration_ms,
      stderr_captured: true
    ]
  end

  defmodule Result do
    @moduledoc """
    A finished pipeline as data.

    `rc` is the LAST stage's status, which is what a shell would have given
    you; it is here for familiarity, not for judgement. Ask `ok?/1` (every
    stage zero) before believing a pipeline worked.
    """
    @type t :: %__MODULE__{
            stages: [Stage.t(), ...],
            out: binary(),
            rc: non_neg_integer() | nil,
            duration_ms: non_neg_integer(),
            timed_out: boolean()
          }
    defstruct [:stages, :out, :rc, :duration_ms, timed_out: false]
  end

  defmodule Error do
    @moduledoc "Raised by `IxMcp.Stdlib.Sh.ok!/1` when any stage exited nonzero."
    defexception [:message, :result]
  end

  defmodule VerifyError do
    @moduledoc "Raised by `IxMcp.Stdlib.Sh.mutate/2` when a postcondition did not hold."
    # `mutation_error` is set only when the mutation itself raised, and
    # `outcomes` carries every clause (not just the failures) so a caller can
    # tell "refused but the world moved" from "refused and nothing moved".
    defexception [:message, :label, :failures, :outcomes, :mutation_error]
  end

  defmodule Watch do
    @moduledoc "A pattern shown to match a positive fixture and reject a negative one."
    @type t :: %__MODULE__{label: binary(), pattern: Regex.t()}
    @enforce_keys [:label, :pattern]
    defstruct [:label, :pattern]
  end

  @doc """
  One command, from an ARGV LIST.

  Options ride to the spawn: `cd:` (defaults to the kernel's launch dir, never
  the movable OS cwd) and `env:` as a list of `{name, value}` pairs.

  Append variables as list elements. Interpolating inside `~w` re-splits the
  result on whitespace, so a path or pattern with a space silently becomes two
  arguments:

      Sh.cmd(~w(rg -o --) ++ [pattern, path])
  """
  @spec cmd([binary(), ...], keyword()) :: Step.t()
  def cmd(argv, opts \\ [])

  def cmd(argv, opts) when is_list(argv) and argv != [] do
    Enum.each(argv, &validate_word!/1)

    %Step{argv: argv, opts: opts}
  end

  def cmd(argv, _opts) when is_binary(argv) do
    raise ArgumentError, """
    Sh.cmd/2 takes an argv list, never a command string; got #{inspect(argv)}.

    A string would have to be word-split by a shell, which is the failure this
    module exists to remove. Write the words out and append variables as
    elements:

        Sh.cmd(~w(rg -o --) ++ [pattern, path])

    For a multi-stage pipeline use Sh.pipe/2 or Sh.pipeline/1 rather than a
    "|" inside one string.
    """
  end

  def cmd(argv, _opts),
    do: raise(ArgumentError, "Sh.cmd/2 needs a non-empty argv list, got #{inspect(argv)}")

  @doc """
  Feed `left`'s stdout into `right`'s stdin. Either side may already be a
  pipeline, so `pipe/2` composes left to right.
  """
  @spec pipe(Step.t() | Pipeline.t(), Step.t() | Pipeline.t()) :: Pipeline.t()
  def pipe(left, right), do: %Pipeline{steps: steps(left) ++ steps(right)}

  @doc """
  A pipeline from a list of argv lists (or already-built steps).

      Sh.pipeline([~w(rg -n TODO .), ~w(cut -d: -f1), ~w(uniq -c)])
  """
  @spec pipeline([[binary(), ...] | Step.t(), ...]) :: Pipeline.t()
  def pipeline([_ | _] = stages) do
    %Pipeline{steps: Enum.map(stages, &as_step/1)}
  end

  defp validate_word!(word) when not is_binary(word) do
    raise ArgumentError, "argv words must be strings, got #{inspect(word)}"
  end

  # A NUL is not a character to execve, it is the argv terminator. The child
  # would have received the bytes BEFORE it and run happily on the truncated
  # input -- a clean, wrong answer, which is worse than a crash.
  defp validate_word!(word) do
    case :binary.match(word, <<0>>) do
      {offset, _len} ->
        raise ArgumentError, """
        argv word contains a NUL byte at offset #{offset}. execve terminates
        each argv string at the first NUL, so the child would have run on the
        truncated prefix and reported success on the wrong input:
        #{inspect(word, printable_limit: 120)}
        """

      :nomatch ->
        :ok
    end

    if byte_size(word) >= @max_arg_strlen do
      raise ArgumentError, """
      argv word is #{byte_size(word)} bytes; a single argv string must be UNDER
      #{@max_arg_strlen} bytes. The kernel test is strlen(arg) < MAX_ARG_STRLEN,
      so #{@max_arg_strlen} itself already fails, and a bigger ARG_MAX on the
      host does not raise it. The spawn would have failed with a bare
      "Argument list too long".

      argv carries ARGUMENTS. A body belongs on stdin, which has no such limit:

          Sh.run(Sh.cmd(~w(tee /tmp/out)), stdin: body)
          Sh.run(pipeline, stdin: {:file, path})
      """
    end

    :ok
  end

  defp as_step(%Step{} = step), do: step
  defp as_step(argv) when is_list(argv), do: cmd(argv)
  # Anything else -- notably a bare shell STRING sitting in a pipeline list --
  # goes to cmd/2 so it gets cmd/2's named refusal naming the argv-list fix,
  # rather than a bare FunctionClauseError from this private helper.
  defp as_step(other), do: cmd(other)

  defp steps(%Step{} = step), do: [step]
  defp steps(%Pipeline{steps: steps}), do: steps

  @doc """
  Run a step or pipeline and return a `Result`.

  Never raises for a command failure -- a missing executable, a nonzero exit
  and a timeout all come back as fields. (A refused git mutation or a
  malformed argv still raises: those are the caller's bug, not the world's.)

  Options:

    * `timeout_ms:` -- default #{@default_timeout_ms}. On expiry every stage's
      process tree is killed, `timed_out: true` is set, and stages that never
      exited carry `rc: nil`.
    * `out:` -- `:collect` (default) returns the last stage's stdout in
      `result.out`. `{:file, path}` sends it straight to a file instead, for
      output too big to want in memory; `result.out` is then `""`.
    * `stdin:` -- feed the FIRST stage's stdin: an iodata body, or
      `{:file, path}` to stream a file in without it passing through this VM.
      This is where a body too large for argv goes (see MAX_ARG_STRLEN in the
      moduledoc). The default is `/dev/null` rather than an open pipe, and that
      is deliberate: a port's stdin is a pipe the BEAM never closes, so a
      pathless `rg` or `grep` at the head of a pipeline would wait forever for
      input that never ends.
  """
  @spec run(Step.t() | Pipeline.t(), keyword()) :: Result.t()
  def run(pipeline, opts \\ [])
  def run(%Step{} = step, opts), do: run(%Pipeline{steps: [step]}, opts)

  def run(%Pipeline{steps: [_ | _] = steps}, opts) do
    started = System.monotonic_time(:millisecond)
    dir = scratch_dir(opts)

    try do
      precheck!(steps, opts)

      steps
      |> spawn_stages(dir, opts)
      |> collect(opts)
      |> build_result(dir, started)
    after
      cleanup_scratch(dir)
    end
  end

  defp cleanup_scratch(dir) do
    case File.rm_rf(dir) do
      {:ok, _removed} ->
        :ok

      {:error, reason, path} ->
        # Discarding this result was a silent failure in the module whose entire
        # thesis is that silent failures are the enemy. A leaked scratch dir is
        # 0700 and unguessable, so it is a leak and not an exposure -- worth a
        # log line, not worth failing a run that otherwise succeeded.
        Logger.warning(
          "Sh: scratch dir #{dir} not fully removed (#{path}): #{:file.format_error(reason)}"
        )
    end
  end

  # Every stage is checked before ANY stage spawns. Checking inside the spawn
  # loop would mean a refused or mistyped LATER stage leaves the earlier ones
  # already running and blocked on a FIFO nobody will ever open, turning a
  # caller's typo into a full timeout instead of an immediate error.
  defp precheck!(steps, opts) do
    validate_opts!(opts)

    Enum.each(steps, fn %Step{argv: [exe | args]} = step ->
      cd = Keyword.get(step.opts, :cd, Cmd.launch_cwd())
      validate_cd!(cd)
      validate_env!(step.opts)
      # `Step` is a public struct, so `%{step | argv: [...]}` and a hand-built
      # `%Step{}` both reach here without ever passing through cmd/2. Validating
      # words ONLY in cmd/2 left the NUL guard bypassable in one line, and a NUL
      # in argv returns the clean, confident, wrong answer (a word "pat\0tern"
      # runs as "pat") that the guard's own comment says it exists to prevent.
      Enum.each(step.argv, &validate_word!/1)
      child_env = Keyword.get(step.opts, :env, [])
      GitGuard.check!(exe, args, cd, child_env)
      guard_indirect!(exe, args, cd, child_env)
    end)
  end

  # `Sh.cmd(~w(sh -c) ++ ["cd P && git add -A"])` has argv[0] == "sh", so the git
  # guard never sees a git command at all: measured ALLOWED against a protected
  # checkout while the direct `git add -A` and `git -C P add -A` forms were both
  # refused. IxMcp.Cmd closes this on its own shell path with check_script!/2,
  # and this seam needs the same or the refusal is one `sh -c` from decorative.
  # Two ways the first version of this was still one letter from decorative, both
  # measured against /bin/sh on this host:
  #
  #   * it recognised only an argv word EQUAL to "-c", while `sh -ec`, `bash -lc`
  #     and `sh -xc` all execute their next operand as a script. Any short-flag
  #     CLUSTER ending in `c` is a script flag; `--command` is the long spelling.
  #   * it read only argv[0], so a single wrapper hid everything behind it.
  #
  # A wrapper execs its operand, so the guard has to look THROUGH it rather than
  # allow it. Allowing is the silent-wrong-answer shape this module exists to kill.
  defp guard_indirect!(exe, args, cd, env) do
    base = Path.basename(exe)

    cond do
      base in @shells -> guard_shell_args!(args, cd)
      base in @wrappers -> guard_wrapped!(args, cd, env)
      true -> :ok
    end
  end

  defp guard_shell_args!(args, cd) do
    case script_operand(args) do
      {:ok, script} -> GitGuard.check_script!(script, cd)
      :none -> :ok
    end
  end

  # Scan EVERY position behind a wrapper instead of guessing which word is the
  # program: `timeout 5 git add -A` and `nice -n 5 git push` both put an operand
  # between the wrapper and the program, so "the first word that is not a flag"
  # picks the number and lets git straight through. Deliberately conservative --
  # a word literally named `git` followed by a mutating subcommand aimed at a
  # protected checkout is refused even if it was data, because a false refusal is
  # recoverable in one edit and a bypass is not recoverable at all.
  defp guard_wrapped!([], _cd, _env), do: :ok

  defp guard_wrapped!([arg | rest], cd, env) do
    GitGuard.check!(arg, rest, cd, env)
    if Path.basename(arg) in @shells, do: guard_shell_args!(rest, cd)
    guard_wrapped!(rest, cd, env)
  end

  # A short-flag cluster ending in `c` takes the NEXT operand as the script.
  defp script_flag?(arg), do: Regex.match?(~r/^-[A-Za-z]*c$/, arg)

  defp script_operand([]), do: :none

  defp script_operand([arg | rest]) do
    cond do
      script_flag?(arg) or arg == "--command" ->
        case rest do
          [script | _ignored] when is_binary(script) -> {:ok, script}
          [] -> :none
        end

      String.starts_with?(arg, "--command=") ->
        {:ok, String.replace_prefix(arg, "--command=", "")}

      true ->
        script_operand(rest)
    end
  end

  # An unrecognised option value used to fall back to the default, which is the
  # silent-wrong-answer shape this module exists to kill: `out: {:path, p}` sent
  # the output to the reply instead of the file and nothing said so. And a
  # timeout that was neither a positive integer nor :infinity raised from the
  # deadline arithmetic AFTER the stages were running, leaking every one.
  @run_opts [:timeout_ms, :out, :stdin, :scratch_root]

  defp validate_opts!(opts) do
    validate_known!(opts, @run_opts, "run/2")
    validate_timeout!(Keyword.get(opts, :timeout_ms, @default_timeout_ms))
    validate_out!(Keyword.get(opts, :out, :collect))
    validate_stdin!(Keyword.fetch(opts, :stdin))
  end

  # An unknown key used to be ignored in silence, which is this module's own
  # cardinal sin wearing a keyword list: `timout_ms: 5` kept the 600_000 ms
  # default and said nothing, so a caller who thought they had set a 5 ms timeout
  # got a confident wrong answer ten minutes later.
  defp validate_known!(opts, allowed, where) do
    case Keyword.keys(opts) -- allowed do
      [] ->
        :ok

      unknown ->
        raise ArgumentError,
              "#{where}: unknown option(s) #{inspect(unknown)}. " <>
                "Known: #{inspect(allowed)}. A silently ignored option is a wrong answer " <>
                "delivered confidently."
    end
  end

  defp validate_timeout!(:infinity), do: :ok
  defp validate_timeout!(ms) when is_integer(ms) and ms > 0, do: :ok

  defp validate_timeout!(other) do
    raise ArgumentError,
          "timeout_ms: must be a positive integer or :infinity, got #{inspect(other)}"
  end

  defp validate_out!(:collect), do: :ok

  defp validate_out!({:file, path}) when is_binary(path) do
    validate_io_path!("out", path)
    validate_out_parent!(path)
  end

  defp validate_out!(other) do
    raise ArgumentError, "out: must be :collect or {:file, path}, got #{inspect(other)}"
  end

  defp validate_stdin!(:error), do: :ok

  defp validate_stdin!({:ok, {:file, path}}) when is_binary(path) do
    validate_io_path!("stdin", path)
    validate_stdin_file!(path)
  end

  defp validate_stdin!({:ok, body}) when is_binary(body), do: :ok

  # A list that is not iodata surfaced as a File.Error from inside spawn_stages
  # instead of a named refusal, so iolist_size/1 is the cheapest honest check --
  # and it is the only one that gets IMPROPER lists right, since ["a" | "b"] is
  # perfectly good iodata that a hand-written Enum-based predicate would crash on.
  defp validate_stdin!({:ok, body}) when is_list(body) do
    if iodata_size(body) == :error do
      raise ArgumentError, "stdin: list must be iodata, got #{inspect(body)}"
    end

    :ok
  end

  defp validate_stdin!({:ok, other}) do
    raise ArgumentError, "stdin: must be iodata or {:file, path}, got #{inspect(other)}"
  end

  # A mistyped stdin path used to cost the FULL timeout instead of an error:
  # stage 0 died before opening its outgoing FIFO, so every later stage sat
  # blocked on an open nobody would ever complete. The redirect order now
  # guarantees EOF instead of a deadlock, and this refuses the typo outright.
  defp validate_stdin_file!(path) do
    case File.stat(path) do
      {:ok, %File.Stat{type: :directory}} ->
        raise ArgumentError, "stdin: #{path} is a directory"

      {:ok, _stat} ->
        :ok

      {:error, reason} ->
        raise ArgumentError, "stdin: #{path} is not readable: #{:file.format_error(reason)}"
    end
  end

  # A RELATIVE io path is validated against the BEAM's own OS cwd and then opened
  # relative to the STAGE's cd:, so the directory checked and the directory
  # written are different ones -- `out: {:file, "out.txt"}` with `cd: "/tmp"`
  # validated the package dir and wrote /tmp/out.txt. That is the movable-cwd
  # hazard (#3902) Cmd.launch_cwd/0 exists to refuse, back in through the io
  # options. `stdin:` belongs to stage 0 and `out:` to the last stage, and those
  # two can carry different cd:, so no single base would be correct to resolve
  # against: refuse the ambiguity rather than guess at it.
  #
  # The NUL check belongs here too. A NUL truncates the generated script
  # mid-quote, so sh dies in its PARSER before ANY redirect takes effect -- the
  # stage's own 2> included, which is why its real diagnostic goes to the BEAM
  # console and the stage comes back rc=2 with stderr_captured: false.
  defp validate_io_path!(which, path) do
    cond do
      :binary.match(path, <<0>>) != :nomatch ->
        raise ArgumentError, "#{which}: path #{inspect(path)} contains a NUL byte"

      Path.type(path) != :absolute ->
        raise ArgumentError,
              "#{which}: #{inspect(path)} must be an absolute path -- a relative one is " <>
                "checked against the BEAM's cwd but opened relative to the stage's cd:"

      true ->
        :ok
    end
  end

  defp validate_scratch_root!(root) do
    cond do
      not is_binary(root) ->
        raise ArgumentError, "scratch_root: must be a path, got #{inspect(root)}"

      Path.type(root) != :absolute ->
        raise ArgumentError, "scratch_root: #{inspect(root)} must be an absolute path"

      not File.dir?(root) ->
        raise ArgumentError, "scratch_root: #{inspect(root)} is not a directory"

      true ->
        :ok
    end
  end

  # The rescue RETURNS rather than raising: raising a fresh exception inside a
  # rescue block discards the original stacktrace, and the refusal belongs to the
  # caller's frame anyway.
  defp iodata_size(body) do
    :erlang.iolist_size(body)
  rescue
    ArgumentError -> :error
  end

  defp validate_out_parent!(path) do
    parent = Path.dirname(path)

    case File.stat(parent) do
      {:ok, %File.Stat{type: :directory}} ->
        :ok

      {:ok, %File.Stat{type: other}} ->
        raise ArgumentError, "out: parent #{parent} is not a directory (#{other})"

      {:error, reason} ->
        raise ArgumentError, "out: parent #{parent} is not usable: #{:file.format_error(reason)}"
    end
  end

  # Port env: values are validated deep inside the driver, so a wrong shape (an
  # atom name, a nil value) raises from Port.open with earlier stages already
  # spawned. Refuse it here, while nothing is running.
  defp validate_env!(opts) do
    case Keyword.fetch(opts, :env) do
      :error ->
        :ok

      {:ok, env} when is_list(env) ->
        Enum.each(env, &validate_env_entry!/1)

      {:ok, other} ->
        raise ArgumentError,
              "env: must be a list of {binary_name, binary_value | false}, got #{inspect(other)}"
    end
  end

  defp validate_env_entry!({name, value})
       when is_binary(name) and (is_binary(value) or value == false) do
    validate_env_name!(name)
    validate_env_value!(value)
  end

  defp validate_env_entry!(other) do
    raise ArgumentError,
          "env: entries must be {binary_name, binary_value | false}, got #{inspect(other)}"
  end

  # The shape check alone was not enough, which is the same defect its own
  # comment claims to have closed: a NUL in a name or value, and an `=` inside a
  # name, all passed here and then raised "2nd argument: invalid option in list"
  # from Port.open with earlier stages already spawned. The transactional spawn
  # cleans up, so this was a DIAGNOSIS bug rather than a leak -- the caller got an
  # opaque driver error where a named refusal belongs.
  defp validate_env_name!(name) do
    cond do
      :binary.match(name, <<0>>) != :nomatch ->
        raise ArgumentError, "env: name #{inspect(name)} contains a NUL byte"

      :binary.match(name, "=") != :nomatch ->
        raise ArgumentError,
              "env: name #{inspect(name)} contains `=`, which the port driver rejects as an " <>
                "invalid option mid-spawn rather than as a named refusal"

      true ->
        :ok
    end
  end

  defp validate_env_value!(false), do: :ok

  defp validate_env_value!(value) do
    if :binary.match(value, <<0>>) != :nomatch do
      raise ArgumentError, "env: value for a name contains a NUL byte: #{inspect(value)}"
    end

    :ok
  end

  # Erlang reports a child's failed chdir by exiting with the raw errno, which
  # is indistinguishable from the command's own status, so a bad cd: has to be
  # refused before the spawn rather than diagnosed after it.
  defp validate_cd!(cd) do
    case File.stat(cd) do
      {:ok, %File.Stat{type: :directory}} ->
        :ok

      {:ok, %File.Stat{type: other}} ->
        raise ArgumentError, "cd target #{cd} is not a directory (#{other})"

      {:error, reason} ->
        raise ArgumentError, "cd target #{cd} is not usable: #{:file.format_error(reason)}"
    end
  end

  # One scratch dir per run holds the FIFOs joining the stages and one stderr
  # file per stage. Ports cannot hand a child's stderr back separately -- the
  # only port option is :stderr_to_stdout, which would fold diagnostics into
  # the data stream and destroy the per-stage split this module is for -- so
  # each stage redirects its own fd 2 to its own file.
  # `scratch_root:` exists because the leak tests cannot otherwise be written
  # without flaking: /tmp is a SHARED namespace, so "no ix-sh-* dir appeared"
  # is false whenever any concurrent run -- another async test, or the machine's
  # real kernel in a separate OS process under the same TMPDIR -- starts one
  # between the snapshot and the assertion. A per-run option is immune to
  # neighbours in a way a global config setting would not be: with app env, one
  # async test's root becomes every concurrent test's root too.
  defp scratch_dir(opts) do
    root = Keyword.get(opts, :scratch_root, System.tmp_dir!())
    validate_scratch_root!(root)

    dir =
      Path.join(
        root,
        "ix-sh-" <> Base.url_encode64(:crypto.strong_rand_bytes(12), padding: false)
      )

    # mkdir! not mkdir_p!, and an unguessable name not a counter: on a shared
    # /tmp a PREDICTABLE name plus a p-mkdir that happily succeeds on an
    # existing directory is the classic pre-creation hijack (CWE-377), and what
    # leaks is every stage's stderr. Fail closed, then 0700 before anything
    # lands in it.
    File.mkdir!(dir)
    File.chmod!(dir, 0o700)
    dir
  end

  defp spawn_stages(steps, dir, opts) do
    count = length(steps)
    io = %{out: Keyword.get(opts, :out, :collect), stdin: stdin_path(dir, opts)}

    # Every FIFO exists before any stage spawns: opening one blocks until the
    # other end opens, which is what gives the pipeline shell semantics, but a
    # MISSING one would make the neighbour's redirect fail instead of block.
    for index <- 0..(count - 2)//1, do: mkfifo!(Path.join(dir, "f#{index}"))

    steps
    |> Enum.with_index()
    |> Enum.reduce([], fn {step, index}, spawned ->
      [spawn_one(step, index, count, dir, io, spawned) | spawned]
    end)
    |> Enum.reverse()
  end

  # Spawning is all-or-nothing. Without this, a raise on stage 2 (an OS resource
  # limit, anything precheck cannot see) left stages 0 and 1 running and
  # unreferenced: the caller gets the exception, the processes stay blocked on a
  # FIFO in a scratch dir that is about to be deleted, and nothing reaps them.
  defp spawn_one(step, index, count, dir, io, spawned) do
    %{
      port: spawn_stage(step, index, count, dir, io),
      index: index,
      argv: step.argv,
      spawned_at: System.monotonic_time(:millisecond)
    }
  rescue
    error ->
      Enum.each(spawned, &kill_port(&1.port))
      reraise error, __STACKTRACE__
  catch
    kind, reason ->
      Enum.each(spawned, &kill_port(&1.port))
      :erlang.raise(kind, reason, __STACKTRACE__)
  end

  # A body reaches the pipeline through a file, never through argv: the spill
  # gives stage 1 a real EOF for free, and no size limit applies.
  defp stdin_path(dir, opts) do
    case Keyword.get(opts, :stdin) do
      nil ->
        "/dev/null"

      {:file, path} ->
        path

      body ->
        spilled = Path.join(dir, "stdin")
        File.write!(spilled, body)
        spilled
    end
  end

  defp mkfifo!(path) do
    # Internal plumbing, so System.cmd is fine here: mkfifo never reads stdin,
    # which is the hazard IxMcp.Cmd's /dev/null redirect exists for.
    case System.cmd("mkfifo", [path], stderr_to_stdout: true) do
      {_out, 0} -> :ok
      {out, rc} -> raise "mkfifo #{path} failed (rc=#{rc}): #{String.trim(out)}"
    end
  end

  # `sh -c 'exec "$0" "$@" <in >out 2>err' cmd arg1 arg2` -- the argv words
  # arrive as positional parameters, so no shell parsing ever touches them,
  # and `exec` replaces the shell so cancellation still sees one process tree.
  # Same trick IxMcp.Cmd uses for its /dev/null stdin.
  defp spawn_stage(%Step{argv: [exe | args]} = step, index, count, dir, io) do
    cd = Keyword.get(step.opts, :cd, Cmd.launch_cwd())

    script = ~s(exec "$0" "$@" ) <> redirects(index, count, dir, io)

    port_opts =
      [:binary, :exit_status, :stream, :hide, {:args, ["-c", script, exe | args]}, {:cd, cd}] ++
        env_opt(step.opts)

    Port.open({:spawn_executable, @shell}, port_opts)
  end

  defp env_opt(opts) do
    case Keyword.fetch(opts, :env) do
      {:ok, env} -> [{:env, env}]
      :error -> []
    end
  end

  # POSIX applies redirections LEFT TO RIGHT, so this order is load-bearing
  # twice over.
  #
  # fd 2 goes FIRST. Anything opened before it reports its own failure to the
  # stderr sh inherited -- the BEAM's console -- where no Result can ever see
  # it. A failing `>out` used to surface as rc=1 with EMPTY stderr, and the
  # broken pipe it caused was then reported against the innocent upstream stage.
  #
  # The FIFOs go before the caller's redirects. A stage that dies on its own
  # `<in` has then already opened its outgoing FIFO, so the downstream stage
  # gets a prompt EOF; with the caller's redirect first, one missing stdin file
  # cost the entire timeout in a silent deadlock.
  defp redirects(index, count, dir, io) do
    err = "2>" <> shell_quote(Path.join(dir, "e#{index}"))
    fifo_in = if index > 0, do: " <" <> shell_quote(Path.join(dir, "f#{index - 1}")), else: ""

    fifo_out =
      if index < count - 1, do: " >" <> shell_quote(Path.join(dir, "f#{index}")), else: ""

    # Stage 0 reads the caller's stdin, defaulting to /dev/null and never the
    # port's own pipe: the BEAM never closes that pipe, so a pathless rg at the
    # head of a pipeline would block forever on an EOF that never comes.
    caller_in = if index == 0, do: " <" <> shell_quote(io.stdin), else: ""

    caller_out =
      case {index == count - 1, io.out} do
        {true, {:file, path}} -> " >" <> shell_quote(path)
        _other -> ""
      end

    err <> fifo_in <> fifo_out <> caller_in <> caller_out
  end

  # Redirect targets are the only text baked into the script. Our own paths
  # are tame, but `out: {:file, path}` comes from the caller.
  defp shell_quote(path), do: "'" <> String.replace(path, "'", ~S('\'')) <> "'"

  defp collect(spawned, opts) do
    deadline =
      case Keyword.get(opts, :timeout_ms, @default_timeout_ms) do
        :infinity -> :infinity
        timeout -> System.monotonic_time(:millisecond) + timeout
      end

    last = List.last(spawned).port
    pending = Map.new(spawned, fn stage -> {stage.port, stage} end)
    collect_loop(pending, %{}, [], last, deadline, spawned)
  end

  # Every clause below is keyed to a port THIS run owns. They used to match an
  # unbound `_port`, which in a long-lived cell process silently drained mail
  # belonging to any other port the cell had opened: a foreign port's payload
  # AND its exit status both vanished, with message_queue_len back at 0 and
  # nothing anywhere saying a message had been thrown away. A REPL kernel whose
  # cells run arbitrary code is precisely where that bites.
  defp collect_loop(pending, exits, acc, last, deadline, spawned) when map_size(pending) > 0 do
    remaining = remaining_ms(deadline)

    if remaining == 0 do
      Enum.each(Map.keys(pending), &kill_port/1)
      {:timed_out, exits, acc, spawned}
    else
      receive do
        {port, {:data, data}} when port == last ->
          collect_loop(pending, exits, [acc | data], last, deadline, spawned)

        {port, {:data, _data}} when is_map_key(pending, port) ->
          # A non-final stage's stdout goes to a FIFO, so it sends no data;
          # anything arriving from a stage WE own is not output we own.
          collect_loop(pending, exits, acc, last, deadline, spawned)

        {port, {:exit_status, rc}} when is_map_key(pending, port) ->
          at = System.monotonic_time(:millisecond)

          collect_loop(
            Map.delete(pending, port),
            Map.put(exits, port, {rc, at}),
            acc,
            last,
            deadline,
            spawned
          )
      after
        remaining ->
          Enum.each(Map.keys(pending), &kill_port/1)
          {:timed_out, exits, acc, spawned}
      end
    end
  end

  defp collect_loop(_pending, exits, acc, _last, _deadline, spawned),
    do: {:ok, exits, acc, spawned}

  # Kill the whole descendant tree, not just `sh`: `exec` means sh IS the
  # command, but a command that forked children of its own would otherwise
  # outlive the run as orphans.
  defp kill_port(port) do
    case Port.info(port, :os_pid) do
      {:os_pid, os_pid} -> kill_tree_or_signal(os_pid)
      nil -> :ok
    end

    close_port(port)
    drain(port)
  end

  # `if Port.info(port) != nil, do: Port.close(port)` is CHECK-THEN-ACT, and the
  # SIGKILL just above is what closes the window: the port gets reaped between
  # the check and the close, and port_close/1 RAISES ArgumentError on an
  # already-closed port. That turned a timeout back into an exception -- the one
  # thing this module promises never happens -- in 2 of 144 runs under 24-way
  # concurrency, and on the spawn_one rescue path it REPLACED the original
  # failure with a bare "argument error", destroying the diagnosis the
  # transactional spawn exists to preserve. No guard can win this race, so the
  # close is unconditional and its refusal absorbed.
  @doc false
  @spec close_port(port() | term()) :: :ok
  def close_port(port) do
    Port.close(port)
    :ok
  rescue
    ArgumentError -> :ok
  end

  # OsProc.kill_tree finds descendants by shelling out to `pgrep`, which is NOT
  # present in every environment -- the nix check sandbox has no procps, and
  # there System.cmd raises :enoent. Letting that through would break this
  # module's central promise: a timeout is a FIELD, not an exception. So the
  # descendant sweep is best effort, and the fallback signals the stage directly
  # through the shell's `kill` BUILTIN, which needs no binary on PATH and cannot
  # be missing wherever /bin/sh already runs (this module's hard dependency).
  # Deep descendants can survive that degraded path; a raised timeout cannot.
  # `sweep` is injectable for ONE reason: the fallback is the whole basis of the
  # claim that a timeout cannot raise, and on a host that HAS pgrep the fallback
  # is unreachable, while on the host where it does run (the nix check sandbox)
  # the verifying assertion is the one that gets skipped. So the contract is
  # tested directly with a sweeper that raises.
  @doc false
  @spec kill_tree_or_signal(non_neg_integer(), (non_neg_integer() -> term())) :: :ok
  def kill_tree_or_signal(os_pid, sweep \\ &OsProc.kill_tree/1) do
    sweep.(os_pid)
    :ok
  rescue
    _error -> signal_directly(os_pid)
  catch
    _kind, _reason -> signal_directly(os_pid)
  end

  defp signal_directly(os_pid) do
    System.cmd(@shell, ["-c", "kill -9 #{os_pid}"], stderr_to_stdout: true)
    :ok
  rescue
    _error -> :ok
  catch
    _kind, _reason -> :ok
  end

  defp remaining_ms(:infinity), do: :infinity
  defp remaining_ms(deadline), do: max(deadline - System.monotonic_time(:millisecond), 0)

  # A timed-out run used to leave its ports' unread {:data, _} and
  # {:exit_status, _} messages in the mailbox forever. A cell process is
  # long-lived and runs many pipelines, so that is both an unbounded leak and a
  # booby trap: a LATER receive could match a message this run abandoned.
  defp drain(port) do
    receive do
      {^port, _message} -> drain(port)
    after
      0 -> :ok
    end
  end

  defp build_result({status, exits, acc, spawned}, dir, started) do
    stages =
      Enum.map(spawned, fn stage ->
        {rc, exited_at} = Map.get(exits, stage.port, {nil, System.monotonic_time(:millisecond)})
        {stderr, bytes, truncated, captured} = read_stderr(dir, stage.index)

        %Stage{
          argv: stage.argv,
          rc: rc,
          stderr: stderr,
          stderr_bytes: bytes,
          stderr_truncated: truncated,
          stderr_captured: captured,
          duration_ms: exited_at - stage.spawned_at
        }
      end)

    %Result{
      stages: stages,
      out: IO.iodata_to_binary(acc),
      rc: stages |> List.last() |> Map.fetch!(:rc),
      duration_ms: System.monotonic_time(:millisecond) - started,
      timed_out: status == :timed_out
    }
  end

  # "No stderr file" and "the stage printed nothing" used to be the same empty
  # string, which is what made a failed redirect undiagnosable: the one stage
  # whose fd 2 never opened looked like the quiet, healthy one.
  defp read_stderr(dir, index) do
    path = Path.join(dir, "e#{index}")

    case File.stat(path) do
      {:ok, %File.Stat{size: size}} ->
        # Deriving `truncated` from the RAW size alone reported
        # stderr_truncated: false on a stderr that had in fact been cut, because
        # sanitizing EXPANDS -- each invalid byte becomes a four-character escape,
        # so a stderr comfortably under the cap can still lose its head on the way
        # out. That is precisely the "a capped stderr is never mistaken for a short
        # one" promise the Stage struct's own comment makes.
        {capped, cut_by_cap} = sanitize_capped(read_tail(path, size))
        {capped, size, size > @stderr_cap or cut_by_cap, true}

      {:error, _reason} ->
        {"", 0, false, false}
    end
  end

  # Reading the whole file and capping afterwards made a stage that wrote 256 MB
  # to stderr cost 256 MB of binary memory to KEEP 16 KiB -- per stage, in the
  # module whose docs advertise a cap -- so a runaway loop's diagnostics could
  # OOM the node. Seek to the tail instead and read only what is kept.
  defp read_tail(path, size) do
    case File.open(path, [:read, :binary]) do
      {:ok, fd} ->
        try do
          with {:ok, _position} <- :file.position(fd, {:bof, max(size - @stderr_cap, 0)}),
               {:ok, data} <- :file.read(fd, @stderr_cap) do
            data
          else
            _empty_or_error -> ""
          end
        after
          File.close(fd)
        end

      {:error, _reason} ->
        ""
    end
  end

  # A byte-exact tail can begin INSIDE a multi-byte character, and an invalid
  # binary raises {:invalid_byte, _} from JSON.encode! on the reply path -- the
  # #3538 failure IxMcp.UTF8 exists to prevent, which would take down a whole
  # connection because some stage printed an arrow.
  #
  # Order matters and used to be wrong: capping BEFORE sanitizing let each
  # invalid byte expand to four characters afterwards, so a 16 KiB tail came back
  # as 37,579 bytes while report/1 still announced "tail 16384B". Sanitize first,
  # then cap on a character boundary with UTF8.truncate/2 -- which is also what
  # retires the hand-rolled continuation-byte trim this used to carry.
  #
  # truncate_tail/2, NOT truncate/2: the prefix form silently ate the end of the
  # diagnostic. Escaping one partial leading character adds 3 bytes, which pushed
  # the sanitized tail 3 bytes over the cap, and cutting those from the FRONT-kept
  # prefix removed the last 3 bytes of the final message instead. The last line of
  # stderr is the whole reason anyone reads stderr.
  # Returns the capped text AND whether capping dropped anything, so the caller
  # never has to infer truncation from a byte count taken before sanitizing.
  defp sanitize_capped(bytes) do
    sanitized = UTF8.sanitize(bytes)
    capped = UTF8.truncate_tail(sanitized, @stderr_cap)
    {capped, byte_size(sanitized) > byte_size(capped)}
  end

  @doc """
  True when EVERY stage exited 0.

  This is the question a shell cannot answer for you, and the reason
  `result.rc` alone is not a verdict.
  """
  @spec ok?(Result.t()) :: boolean()
  def ok?(%Result{stages: stages}), do: Enum.all?(stages, &(&1.rc == 0))

  @doc """
  The last stage's stdout when every stage exited 0; raises `Sh.Error`
  carrying the whole stage table otherwise.

  Accepts a `Result` or an unrun step/pipeline, so a verification read is one
  expression: `Sh.ok!(Sh.cmd(~w(jj log -r main)))`. An unrun step takes the same
  options as `run/2`: `Sh.ok!(step, timeout_ms: 30_000)`.
  """
  @spec ok!(Result.t() | Step.t() | Pipeline.t(), keyword()) :: binary()
  def ok!(runnable, opts \\ [])

  def ok!(%Result{} = result, _opts) do
    if ok?(result) do
      result.out
    else
      raise Error,
        message: "pipeline did not succeed in every stage:\n" <> report(result),
        result: result
    end
  end

  def ok!(%Step{} = step, opts), do: ok!(run(step, opts))
  def ok!(%Pipeline{} = pipeline, opts), do: ok!(run(pipeline, opts))

  @doc """
  The per-stage table: rc, duration, argv and stderr for every stage. This is
  what `ok!/1` puts in the raise, and what to print when a pipeline surprises
  you.
  """
  @spec report(Result.t()) :: binary()
  def report(%Result{} = result) do
    header = if result.timed_out, do: "TIMED OUT after #{result.duration_ms}ms\n", else: ""

    body =
      result.stages
      |> Enum.with_index(1)
      |> Enum.map_join("\n", fn {stage, position} -> stage_line(position, stage) end)

    header <> body
  end

  defp stage_line(position, %Stage{} = stage) do
    # "never exited" was a lie about the common case: these stages DID exit, we
    # SIGKILLed them on timeout and then discarded the status by closing the port.
    rc = if is_integer(stage.rc), do: "rc=#{stage.rc}", else: "rc=none (killed on timeout)"
    head = "  #{position}. #{rc} #{stage.duration_ms}ms  #{Enum.join(stage.argv, " ")}"

    case {stage.stderr_captured, String.trim(stage.stderr)} do
      {false, _nothing} ->
        head <>
          "\n     stderr: NOT CAPTURED -- this stage died before its own" <> " 2> took effect"

      {true, ""} ->
        head

      {true, stderr} ->
        head <> "\n     stderr#{truncation_note(stage)}: #{indent(stderr)}"
    end
  end

  defp truncation_note(%Stage{stderr_truncated: false}), do: ""

  defp truncation_note(%Stage{} = stage) do
    # The KEPT text, not the cap: for a sanitized stderr the two differ, and
    # announcing the cap while showing fewer bytes is a small confident lie.
    " (tail #{byte_size(stage.stderr)}B of #{stage.stderr_bytes}B)"
  end

  defp indent(text), do: String.replace(text, "\n", "\n       ")

  @doc """
  Run a mutation, then check postconditions against FRESH reads of the world.

  Clauses after `verify` run only after the mutation block finishes, and they
  must go ask the world again -- issue the command a second time, re-read the
  file. Reusing the mutation's own captured output proves nothing: the whole
  failure mode is a mutation whose output claims success while nothing moved.

      Sh.mutate "advance the bookmark" do
        Sh.cmd(~w(jj bookmark set main -r @)) |> Sh.run()
      verify
        Sh.ok!(Sh.cmd(~w(jj log -r main --no-graph -T commit_id))) == expected
        File.exists?(marker)
      end

  Returns the mutation block's own value when every clause holds. Raises
  `VerifyError` otherwise, naming the failing clause's source and, for a
  comparison, each side's value. A clause that raises counts as a failure and
  the exception message rides along -- a verification that cannot run has not
  passed. A mutation that raises ALWAYS raises, but the postcondition verdict
  is reported with it, because "it refused and the world moved anyway" and "it
  refused and nothing moved" call for opposite responses.

  Only `true` and `:ok` count as holding. This is fail-closed and deliberate,
  and it has a sharp edge worth knowing: `verify File.read(path)` FAILS on
  `{:ok, "contents"}`, while `verify File.write(path, x)` passes on `:ok`.
  Write a comparison or a predicate, not a call whose success is a tagged tuple.

  Two things are refused at compile time rather than documented and hoped for:
  a verify clause that mentions a name the mutation half bound (it is reading
  the mutation's memory instead of the world), and a mutation half with no call
  in it at all (nothing can have changed, so the clauses are plain assertions).
  """
  defmacro mutate(label, do: block) do
    {mutation, checks} = split_verify(block)
    refuse_reused_bindings!(mutation, checks)

    quote do
      # The mutation runs inside try/rescue because a mutation that RAISES is
      # this macro's whole motivating scenario: a submit that comes back
      # "refused" may already have landed, and the verdict has to live in a
      # fresh read of the world. Leaving the raise unguarded meant the one case
      # that most needs the postconditions was the one case that skipped them.
      {mutation_outcome, mutation_value} =
        try do
          {:ok, unquote({:__block__, [], mutation})}
        rescue
          error -> {{:raised, error}, nil}
        end

      outcomes = unquote(Enum.map(checks, &check_quoted/1))
      failed = Enum.reject(outcomes, &(&1.ok in [true, :ok]))

      unquote(__MODULE__).resolve_mutation(
        unquote(label),
        mutation_outcome,
        mutation_value,
        failed,
        outcomes
      )
    end
  end

  @doc false
  @spec resolve_mutation(binary(), :ok | {:raised, Exception.t()}, term(), [map()], [map()]) ::
          term()
  def resolve_mutation(_label, :ok, value, [], _outcomes), do: value

  def resolve_mutation(label, :ok, _value, failed, _outcomes) do
    raise VerifyError,
      message:
        "mutation #{inspect(label)} ran, but its postconditions do not hold:\n" <>
          format_failures(failed),
      label: label,
      failures: failed
  end

  # A mutation that raised ALWAYS raises: swallowing a live exception because the
  # world happens to look right would be a worse lie than the one this macro
  # exists to kill. But the postcondition verdict rides along either way, because
  # "it refused AND the world moved" (do not retry) and "it refused and nothing
  # moved" (safe to retry) are the two answers the caller actually needs, and
  # they are indistinguishable from the exception alone.
  def resolve_mutation(label, {:raised, error}, _value, failed, outcomes) do
    verdict =
      if failed == [] do
        "Its postconditions all HOLD, so the mutation may well have taken effect " <>
          "despite the error -- do not blindly retry it."
      else
        "Its postconditions do NOT hold:\n" <> format_failures(failed)
      end

    raise VerifyError,
      message:
        "mutation #{inspect(label)} RAISED #{inspect(error.__struct__)}: " <>
          Exception.message(error) <> "\n" <> verdict,
      label: label,
      failures: failed,
      outcomes: outcomes,
      mutation_error: error
  end

  # `verify` is not a block keyword the parser knows, so it arrives as a bare
  # variable node inside the do-block. Splitting on it is what makes the
  # mutate/verify surface possible at all.
  defp split_verify({:__block__, _meta, exprs}) do
    case Enum.split_while(exprs, &(not verify_marker?(&1))) do
      {_mutation, []} ->
        raise_missing_verify()

      {[], _rest} ->
        raise ArgumentError,
              "Sh.mutate needs a mutation before `verify`: with an empty mutation half " <>
                "the checks are plain assertions, and a passing run proves nothing changed."

      {mutation, [_marker | checks]} when checks != [] ->
        refuse_inert_mutation!(mutation)
        {mutation, checks}

      {_mutation, [_marker]} ->
        raise ArgumentError, "Sh.mutate needs at least one clause after `verify`"
    end
  end

  defp split_verify(_single_expression), do: raise_missing_verify()

  # The docstring forbids reusing the mutation's own captured output as evidence,
  # and that was doctrine only: `r = Sh.run(...)` followed by `verify Sh.ok?(r)`
  # PASSED with nothing mutated, which is exactly the shape being forbidden. This
  # repo's own law is that a freeze must be mechanical, and this one is checkable
  # at compile time -- a verify clause that mentions a name the mutation half
  # bound is reading the mutation's memory instead of the world.
  defp refuse_reused_bindings!(mutation, checks) do
    bound = Enum.reduce(mutation, MapSet.new(), &collect_bound/2)
    used = Enum.reduce(checks, MapSet.new(), &collect_used/2)
    shared = MapSet.intersection(bound, used) |> Enum.sort()

    if shared != [] do
      names = Enum.map_join(shared, ", ", &("`" <> Atom.to_string(&1) <> "`"))

      raise ArgumentError,
            "Sh.mutate: the verify half reads " <>
              names <>
              ", bound by the mutation half. A postcondition has to go ask the " <>
              "world again -- re-read the file, re-issue the query -- because the " <>
              "failure this macro exists to catch is a mutation whose own output " <>
              "claims success while nothing moved. Recompute the value after the " <>
              "mutation instead of reusing it."
    end

    :ok
  end

  defp collect_bound(ast, acc) do
    {_ast, names} =
      Macro.prewalk(ast, acc, fn
        {:=, _meta, [lhs, _rhs]} = node, names -> {node, collect_pattern_vars(lhs, names)}
        node, names -> {node, names}
      end)

    names
  end

  defp collect_pattern_vars(lhs, names) do
    {_ast, found} =
      Macro.prewalk(lhs, names, fn
        {name, _meta, context} = node, acc when is_atom(name) and is_atom(context) ->
          {node, if(underscored?(name), do: acc, else: MapSet.put(acc, name))}

        node, acc ->
          {node, acc}
      end)

    found
  end

  defp collect_used(ast, acc) do
    {_ast, names} =
      Macro.prewalk(ast, acc, fn
        {name, _meta, context} = node, used when is_atom(name) and is_atom(context) ->
          {node, if(underscored?(name), do: used, else: MapSet.put(used, name))}

        node, used ->
          {node, used}
      end)

    names
  end

  defp underscored?(name), do: String.starts_with?(Atom.to_string(name), "_")

  # The "empty mutation half" refusal was cosmetic: a literal `:ok` satisfied it,
  # and one of this module's own tests did exactly that. A mutation half with no
  # CALL in it cannot have mutated anything, so the checks after it are plain
  # assertions and a green run proves nothing changed. This is a syntactic floor,
  # not a purity proof -- it cannot tell a mutating call from a pure one.
  defp refuse_inert_mutation!(mutation) do
    if Enum.any?(mutation, &contains_call?/1) do
      :ok
    else
      raise ArgumentError,
            "Sh.mutate: the mutation half contains no call, so nothing can have " <>
              "changed and the verify clauses are plain assertions. A run that " <>
              "passes here proves only that the world was already in the state " <>
              "you wanted."
    end
  end

  @inert_forms [
    :__block__,
    :__aliases__,
    :=,
    :{},
    :%{},
    :%,
    :<<>>,
    :when,
    :"::",
    :fn,
    :->,
    :+,
    :-,
    :*,
    :/,
    :++,
    :--,
    :<>,
    :==,
    :!=,
    :===,
    :!==,
    :>,
    :<,
    :>=,
    :<=,
    :and,
    :or,
    :not,
    :!,
    :&&,
    :||,
    :in
  ]

  defp contains_call?(ast) do
    {_ast, found} =
      Macro.prewalk(ast, false, fn
        {{:., _dot_meta, _target}, _meta, _args} = node, _found ->
          {node, true}

        {form, _meta, args} = node, found when is_atom(form) and is_list(args) ->
          {node, found or form not in @inert_forms}

        node, found ->
          {node, found}
      end)

    found
  end

  defp verify_marker?({:verify, _meta, context}) when is_atom(context), do: true
  defp verify_marker?(_other), do: false

  defp raise_missing_verify do
    raise ArgumentError, """
    Sh.mutate needs a `verify` section: a mutation with no postcondition is
    exactly the shape this macro exists to refuse.

        Sh.mutate "advance the bookmark" do
          Sh.cmd(~w(jj bookmark set main -r @)) |> Sh.run()
        verify
          Sh.ok!(Sh.cmd(~w(jj log -r main --no-graph -T commit_id))) == expected
        end
    """
  end

  @comparisons [:==, :!=, :===, :!==, :=~, :>, :<, :>=, :<=, :in]

  # A comparison is destructured so the failure can print both sides. Anything
  # else is evaluated whole and reported by its source text.
  defp check_quoted({op, _meta, [left, right]} = check) when op in @comparisons do
    source = Macro.to_string(check)
    applied = {op, [], [quote(do: left_value), quote(do: right_value)]}

    quote do
      (fn ->
         try do
           left_value = unquote(left)
           right_value = unquote(right)

           %{
             source: unquote(source),
             ok: unquote(applied),
             left: left_value,
             right: right_value
           }
         rescue
           error -> %{source: unquote(source), ok: false, raised: Exception.message(error)}
         catch
           kind, reason ->
             %{source: unquote(source), ok: false, raised: "#{kind}: #{inspect(reason)}"}
         end
       end).()
    end
  end

  defp check_quoted(check) do
    source = Macro.to_string(check)

    quote do
      (fn ->
         try do
           %{source: unquote(source), ok: unquote(check)}
         rescue
           error -> %{source: unquote(source), ok: false, raised: Exception.message(error)}
         catch
           kind, reason ->
             %{source: unquote(source), ok: false, raised: "#{kind}: #{inspect(reason)}"}
         end
       end).()
    end
  end

  @doc false
  @spec format_failures([map()]) :: binary()
  def format_failures(failures) do
    Enum.map_join(failures, "\n", fn failure ->
      "  FAILED  #{failure.source}\n" <> failure_detail(failure)
    end)
  end

  defp failure_detail(%{raised: message}), do: "          raised: #{message}"

  defp failure_detail(%{left: left, right: right}) do
    "          left:  #{inspect(left, limit: 20, printable_limit: 400)}\n" <>
      "          right: #{inspect(right, limit: 20, printable_limit: 400)}"
  end

  defp failure_detail(%{ok: value}), do: "          evaluated to: #{inspect(value)}"

  @doc """
  Arm a watcher pattern, validating it against both controls at COMPILE time
  when all three are literals, so a blind watcher fails the build.

      watcher = Sh.watch("gate verdict",
                  pattern: ~r/^gate: (PASS|FAIL)/m,
                  must_match: "gate: FAIL 3 checks",
                  must_not_match: "an UNDECLARED refusing instrument REFUSES (rc=1)")

  Raises when either control fails. `arm/2` is the same check returning
  `{:ok, watch} | {:error, reason}` for patterns assembled at runtime.
  """
  defmacro watch(label, opts) do
    validate_controls_at_compile_time!(label, opts)

    quote do
      case unquote(__MODULE__).arm(unquote(label), unquote(opts)) do
        {:ok, watch} -> watch
        {:error, reason} -> raise ArgumentError, reason
      end
    end
  end

  @doc """
  Arm a watcher: refuse the pattern unless it MATCHES `must_match:` and does
  NOT match `must_not_match:`.

  Both controls are mandatory, because a broken instrument and a true negative
  look identical in the output and the broken one is more common. The negative
  control is the one people skip, and it is what catches a pattern built from
  failure vocabulary: `~r/REFUS/` happily matches the name of a passing arm
  called "an UNDECLARED refusing instrument REFUSES".
  """
  @spec arm(binary(), keyword()) :: {:ok, Watch.t()} | {:error, binary()}
  def arm(label, opts) do
    with {:ok, pattern} <- as_regex(Keyword.fetch!(opts, :pattern)) do
      arm_validated(label, pattern, opts)
    end
  end

  defp arm_validated(label, pattern, opts) do
    positive = Keyword.get(opts, :must_match)
    negative = Keyword.get(opts, :must_not_match)

    cond do
      is_nil(positive) or is_nil(negative) ->
        {:error,
         "watcher #{inspect(label)} refused to arm: a control is MISSING. An absent " <>
           "control is not a weaker control, it is no control -- the watcher would arm " <>
           "having never been shown to say YES or to say NO. Both :must_match and " <>
           ":must_not_match are required."}

      blank?(positive) or blank?(negative) ->
        {:error,
         "watcher #{inspect(label)} refused to arm: a control may not be blank. " <>
           "`must_not_match: \"\"` armed the verbatim ~r/REFUS/ vocabulary pattern this " <>
           "module was written to refuse, straight through the front door: every regex " <>
           "fails to match the empty string, so an empty negative control is not a test, " <>
           "it is a rubber stamp."}

      positive == Regex.source(pattern) ->
        {:error,
         "watcher #{inspect(label)} refused to arm: its positive control is byte-equal to " <>
           "the pattern source, which proves only that a regex matches itself. The control " <>
           "has to be a line the WATCHED OUTPUT would really contain."}

      not Regex.match?(pattern, positive) ->
        {:error,
         "watcher #{inspect(label)} refused to arm: #{inspect(pattern)} does not match its " <>
           "positive control #{inspect(positive)}. An instrument never shown to say YES " <>
           "cannot be believed when it says no."}

      Regex.match?(pattern, negative) ->
        {:error,
         "watcher #{inspect(label)} refused to arm: #{inspect(pattern)} ALSO matches its " <>
           "negative control #{inspect(negative)}. A filter that matches its own subject " <>
           "matter is not a filter -- anchor to the runner's verdict line " <>
           ~S|(^FAIL, "status": "passed", build_rc=) rather than to words that appear| <>
           " in the names of passing arms."}

      true ->
        {:ok, %Watch{label: label, pattern: pattern}}
    end
  end

  defp blank?(control), do: not is_binary(control) or String.trim(control) == ""

  @doc "Does an armed watcher match this text?"
  @spec matches?(Watch.t(), binary()) :: boolean()
  def matches?(%Watch{pattern: pattern}, text), do: Regex.match?(pattern, text)

  @doc """
  Poll `probe` until the armed watcher matches.

  Returns `{:matched, captures}`, or `{:ended, text}` when `done?:` reports the
  work reached a terminal state without ever matching, or `{:timeout, text}`.

  `done?:` is what keeps this from guessing about silence. A quiet-timeout
  watcher has to decide how long is too long, and the tail of a step's
  duration distribution is exactly where there is no data -- one gate here has
  a single input that legitimately takes 54 minutes, so any threshold under an
  hour manufactures a false stall. A terminal-state predicate cannot produce
  one: silence with the work still live is just work.
  """
  @spec until(Watch.t(), (-> binary()), keyword()) ::
          {:matched, [binary()]} | {:ended, binary()} | {:timeout, binary()}
  def until(%Watch{} = watch, probe, opts \\ []) when is_function(probe, 0) do
    interval = Keyword.get(opts, :interval_ms, 1_000)
    timeout = Keyword.get(opts, :timeout_ms, 300_000)
    validate_timeout!(timeout)
    validate_interval!(interval)
    done? = Keyword.get(opts, :done?, fn -> false end)
    poll(watch, probe, done?, interval, deadline(timeout))
  end

  # `now + :infinity` is an ArithmeticError, and this is the same defect that
  # collect/2 was already fixed for one function away: a waiter told to wait
  # forever is the most reasonable thing a caller can ask a terminal-state
  # watcher for.
  defp deadline(:infinity), do: :infinity
  defp deadline(ms), do: System.monotonic_time(:millisecond) + ms

  defp validate_interval!(ms) when is_integer(ms) and ms > 0, do: :ok

  defp validate_interval!(other) do
    raise ArgumentError,
          "interval_ms: must be a positive integer (0 is a busy loop that pins a core), " <>
            "got #{inspect(other)}"
  end

  defp poll(watch, probe, done?, interval, deadline) do
    text = probe.()

    # Match FIRST: work that finished and printed its verdict in the same
    # breath must read as matched, not as ended-without-a-verdict.
    cond do
      captures = Regex.run(watch.pattern, text) ->
        {:matched, captures}

      done?.() ->
        # RE-PROBE before answering. `text` was read at T0 and `done?` answers
        # about T1, so work that printed its verdict between the two and then
        # exited came back as {:ended, _} -- "finished without a verdict", which
        # is the NEGATIVE answer about a run that had in fact passed. Ordering
        # match-before-done? within one iteration does not close that window,
        # because the two observations come from different instants.
        final = probe.()

        case Regex.run(watch.pattern, final) do
          nil -> {:ended, final}
          captures -> {:matched, captures}
        end

      remaining_ms(deadline) == 0 ->
        {:timeout, text}

      true ->
        Process.sleep(interval)
        poll(watch, probe, done?, interval, deadline)
    end
  end

  # A watcher that cannot even be built is a REFUSAL to arm, not an exception:
  # the caller of arm/2 is written to branch on {:error, reason}, so raising
  # from here would skip the refusal path entirely and abort the arming code.
  defp as_regex(%Regex{} = regex), do: {:ok, regex}

  defp as_regex(pattern) when is_binary(pattern) do
    case Regex.compile(pattern) do
      {:ok, regex} ->
        {:ok, regex}

      {:error, reason} ->
        {:error,
         "watcher pattern #{inspect(pattern)} is not a valid regex: #{inspect(reason)}. " <>
           "An instrument that cannot be built has not been validated."}
    end
  end

  defp as_regex(other) do
    {:error, "watcher pattern must be a Regex or a string, got #{inspect(other)}"}
  end

  # Compile-time arming for the literal case. Only shapes that can be read off
  # the AST without evaluating anything are considered, so expansion stays
  # side-effect free; anything else is left to the runtime check.
  defp validate_controls_at_compile_time!(label, opts) when is_list(opts) do
    case literal_regex(Keyword.get(opts, :pattern)) do
      {:ok, pattern} ->
        refuse_missing_controls!(label, opts)
        refuse_bad_controls!(label, pattern, opts)

      _not_a_literal_pattern ->
        :ok
    end
  end

  defp validate_controls_at_compile_time!(_label, _opts), do: :ok

  # An ABSENT control used to slip straight through this gate: literal_binary(nil)
  # returns :error, the `with` below read that as "not a literal, cannot judge at
  # compile time", and so the blindest watcher there is -- one carrying NO negative
  # control at all -- compiled fine and then died as a KeyError at arm time. A
  # literal opts list is exactly where absence IS decidable at compile time, which
  # makes it the one case that must not be waved through.
  defp refuse_missing_controls!(label, opts) do
    Enum.each([:must_match, :must_not_match], fn key ->
      if not Keyword.has_key?(opts, key) do
        raise ArgumentError,
              "watcher #{inspect(literal_label(label))} declares no #{inspect(key)}. " <>
                "An absent control is not a weaker control, it is no control: this " <>
                "watcher would arm having never been shown to say YES or to say NO."
      end
    end)
  end

  defp refuse_bad_controls!(label, pattern, opts) do
    with {:ok, positive} <- literal_binary(Keyword.get(opts, :must_match)),
         {:ok, negative} <- literal_binary(Keyword.get(opts, :must_not_match)),
         {:error, reason} <-
           arm(literal_label(label),
             pattern: pattern,
             must_match: positive,
             must_not_match: negative
           ) do
      raise ArgumentError, reason
    else
      _not_literal_or_armed_fine -> :ok
    end
  end

  defp literal_regex({:sigil_r, _meta, [{:<<>>, _bin_meta, [source]}, modifiers]})
       when is_binary(source) do
    Regex.compile(source, List.to_string(modifiers))
  end

  defp literal_regex(_other), do: :error

  defp literal_binary(value) when is_binary(value), do: {:ok, value}
  defp literal_binary(_other), do: :error

  defp literal_label(label) when is_binary(label), do: label
  defp literal_label(_other), do: "watcher"
end
