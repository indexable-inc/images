defmodule IxMcp.Serve do
  @moduledoc """
  Serve an mkapp-scaffolded web app with check-gated hot reload (index#4015).

  `app/2` starts two supervised jobs: the Vite dev server (`npm run dev`,
  always with `--host` so clients on other tailnet machines can reach it)
  and a gate loop watching the app's `staging/` tree. The agent edits
  `staging/` only; once an edit settles, the gate runs `npm run
  check:staging` and, only when green, `npm run promote` (rsync into
  `src/`), which Vite hot-reloads. A red check leaves the last good live
  tree untouched and records the check output where `status/1` shows it.

  Both are ordinary `IxMcp.Jobs` jobs: cancellation kills the whole OS
  process tree (`IxMcp.OsProc`) and their output reads like any run
  (`Jobs.tail/2`).

  Once the dev server prints its port, the advertised URL is shown in a pane
  of the surrounding ix-term session by running `ixterm open <url>`. When
  there is no `ixterm` to run, or it refuses, the reason is logged with the
  URL and the serve keeps working.
  """

  alias IxMcp.Cmd
  alias IxMcp.Jobs
  alias IxMcp.Serve.State

  require Logger

  @poll_ms 150
  # A staging change counts once the tree holds still this long: agents write
  # several files per edit, and checking a half-written tree tests a torn
  # snapshot.
  @settle_ms 300
  @url_timeout_ms 120_000

  @typedoc "What `status/1` returns for a served app."
  @type status :: %{
          dir: String.t(),
          url: String.t(),
          dev_job: String.t(),
          gate_job: String.t(),
          last_check: :ok | :failed | nil,
          last_check_output: String.t() | nil,
          last_error: String.t() | nil,
          checked_at: DateTime.t() | nil,
          promoted_at: DateTime.t() | nil
        }

  @doc """
  Serve the app in `dir`: install deps if missing, start the dev server and
  the staging gate as background jobs, emit the split-view escape, and
  return `status/1`.

  Options: `:host` overrides the advertised host (default: this machine's
  hostname, reachable because vite runs with `--host`); `open: false` leaves
  the URL unopened.
  """
  @spec app(String.t(), keyword()) :: status()
  def app(dir, opts \\ []) do
    dir = Path.expand(dir)
    ensure_app!(dir)
    ensure_deps!(dir)

    State.merge(dir, %{
      last_check: nil,
      last_check_output: nil,
      last_error: nil,
      checked_at: nil,
      promoted_at: nil
    })

    {dev, _out} =
      Jobs.run("IxMcp.Serve.run_dev(#{inspect(dir)})",
        budget: 0,
        intent: "serve: npm run dev (#{dir})"
      )

    url = await_url(dev.id, opts)

    {gate, _out} =
      Jobs.run("IxMcp.Serve.run_gate(#{inspect(dir)})",
        budget: 0,
        intent: "serve: staging gate (#{dir})"
      )

    State.merge(dir, %{url: url, dev_job: dev.id, gate_job: gate.id})

    if Keyword.get(opts, :open, true) do
      with {:error, reason} <- open_url(url) do
        # A serve does not depend on the pane opening -- the URL is the
        # story, and this is the line that tells you to use it.
        Logger.info("serve: #{reason}; open #{url} yourself")
      end
    end

    status(dir)
  end

  @doc "URL, job ids, and last gate outcome for the app served from `dir`."
  @spec status(String.t()) :: status() | {:error, :not_serving}
  def status(dir) do
    dir = Path.expand(dir)

    case State.get(dir) do
      nil -> {:error, :not_serving}
      entry -> Map.put(entry, :dir, dir)
    end
  end

  @doc "Kill the dev server and gate loop (whole OS process trees) for `dir`."
  @spec stop(String.t()) :: :ok | {:error, :not_serving}
  def stop(dir) do
    dir = Path.expand(dir)

    case State.get(dir) do
      nil ->
        {:error, :not_serving}

      entry ->
        for key <- [:gate_job, :dev_job], id = entry[key], is_binary(id) do
          Jobs.cancel(id)
        end

        State.delete(dir)
        :ok
    end
  end

  # -- job bodies (public: they run by name inside `Jobs` cells) -------------

  @doc false
  @spec run_dev(String.t()) :: {Collectable.t(), non_neg_integer()}
  def run_dev(dir) do
    # `into:` streams vite's output line by line to the job's buffer (the
    # group leader), where `await_url/2` reads the printed port.
    Cmd.run("npm", ["run", "dev", "--", "--host"],
      cd: dir,
      stderr_to_stdout: true,
      into: IO.stream(:stdio, :line)
    )
  end

  @doc false
  @spec run_gate(String.t(), keyword()) :: no_return()
  def run_gate(dir, opts \\ []) do
    runner = Keyword.get(opts, :runner, &npm/2)
    gate_loop(dir, signature(dir), runner)
  end

  # -- gate loop --------------------------------------------------------------

  defp gate_loop(dir, sig, runner) do
    sig = dir |> await_change(sig) |> then(&await_settle(dir, &1))
    check_and_promote(dir, runner)
    gate_loop(dir, sig, runner)
  end

  @doc """
  One gate decision: run `check:staging`; promote only when it is green.
  Every outcome is recorded for `status/1`. The `runner` seam ((dir, :check |
  :promote) -> {output, exit_status}) exists for tests; the default runs npm.
  """
  @spec check_and_promote(String.t(), (String.t(), :check | :promote ->
                                         {String.t(), non_neg_integer()})) ::
          :promoted | :rejected | :promote_failed
  def check_and_promote(dir, runner \\ &npm/2) do
    now = DateTime.utc_now()

    case runner.(dir, :check) do
      {check_out, 0} ->
        case runner.(dir, :promote) do
          {_out, 0} ->
            State.merge(dir, %{
              last_check: :ok,
              last_check_output: check_out,
              last_error: nil,
              checked_at: now,
              promoted_at: DateTime.utc_now()
            })

            :promoted

          {promote_out, code} ->
            State.merge(dir, %{
              last_check: :ok,
              last_check_output: check_out,
              last_error: "promote exited #{code}:\n" <> promote_out,
              checked_at: now
            })

            :promote_failed
        end

      {check_out, code} ->
        # Red: the live tree keeps the last good promote; the agent reads
        # the failure from status/1 (or Jobs.tail of the gate job).
        State.merge(dir, %{
          last_check: :failed,
          last_check_output: check_out,
          last_error: "check:staging exited #{code}",
          checked_at: now
        })

        :rejected
    end
  end

  defp npm(dir, :check),
    do: Cmd.run("npm", ["run", "check:staging"], cd: dir, stderr_to_stdout: true)

  defp npm(dir, :promote), do: Cmd.run("npm", ["run", "promote"], cd: dir, stderr_to_stdout: true)

  defp await_change(dir, sig) do
    Process.sleep(@poll_ms)

    case signature(dir) do
      ^sig -> await_change(dir, sig)
      changed -> changed
    end
  end

  defp await_settle(dir, sig) do
    Process.sleep(@settle_ms)

    case signature(dir) do
      ^sig -> sig
      changed -> await_settle(dir, changed)
    end
  end

  @doc false
  @spec signature(String.t()) :: non_neg_integer()
  def signature(dir) do
    dir
    |> Path.join("staging/**")
    |> Path.wildcard(match_dot: true)
    |> Enum.map(fn path ->
      case File.stat(path, time: :posix) do
        {:ok, stat} -> {path, stat.mtime, stat.size}
        {:error, reason} -> {path, reason, 0}
      end
    end)
    |> :erlang.phash2()
  end

  # -- dev server URL ---------------------------------------------------------

  defp await_url(job_id, opts) do
    deadline = System.monotonic_time(:millisecond) + @url_timeout_ms
    poll_url(job_id, opts, deadline)
  end

  defp poll_url(job_id, opts, deadline) do
    case parse_port(Jobs.output(job_id)) do
      {:ok, port} ->
        advertised_url(port, opts)

      :error ->
        cond do
          not Jobs.get(job_id).running ->
            raise "dev server exited before printing a URL:\n" <> Jobs.tail(job_id, 30)

          System.monotonic_time(:millisecond) > deadline ->
            Jobs.cancel(job_id)
            raise "dev server printed no URL in #{@url_timeout_ms}ms:\n" <> Jobs.tail(job_id, 30)

          true ->
            Process.sleep(200)
            poll_url(job_id, opts, deadline)
        end
    end
  end

  @doc false
  @spec parse_port(String.t()) :: {:ok, :inet.port_number()} | :error
  def parse_port(output) do
    # Vite colors the Local line and bolds the port mid-URL; strip SGR
    # sequences before matching.
    clean = String.replace(output, ~r/\e\[[0-9;]*m/, "")

    case Regex.run(~r{Local:\s+https?://[^:/\s]+:(\d+)}, clean) do
      [_, port] -> {:ok, String.to_integer(port)}
      nil -> :error
    end
  end

  defp advertised_url(port, opts) do
    host =
      Keyword.get_lazy(opts, :host, fn ->
        {:ok, name} = :inet.gethostname()
        List.to_string(name)
      end)

    "http://#{host}:#{port}/"
  end

  # -- showing the URL in a pane -----------------------------------------------

  @doc """
  Show `url` in a pane of the surrounding ix-term session.

  Through the `ixterm` CLI, and not by writing an escape. This used to emit
  `ESC ]5522;open-url;<url> BEL` (ix#8185), which ix-term retired in favour of
  a unix-socket control channel: its scanner now answers that escape with
  "ixterm is out of date: OSC 5522 was replaced by the pane control channel"
  rendered into the pane
  (ix `nix/modules/services/host/ix-term/src/osc.rs`). The old code returned
  `:ok` whatever the write did, so a serve reported success while the pane
  showed that message. Two ways to be wrong at once: the wrong protocol, and
  no way to find out.

  `ixterm` rather than the socket protocol spoken from here, because the
  framing, the request shape and the order a pane is resolved in are one
  implementation in ix and a second one in Elixir is one to keep in step. It
  is found on `PATH` rather than pinned as a store path because ix depends on
  index, so index cannot depend back on ix's binaries; that is also why a
  missing `ixterm` has to be an ordinary runtime answer here rather than
  something the build rules out.

  Returns `{:error, reason}` when the URL did not reach a pane, naming what
  refused it. Nothing about a serve depends on the pane opening, but "it
  opened" and "nobody knows" are different answers and the old code gave the
  first for both.

  `find` is the lookup, injected so a test can assert the missing-`ixterm`
  answer without emptying `PATH` out from under every other async test.
  """
  @spec open_url(String.t(), (String.t() -> String.t() | nil)) ::
          :ok | {:error, String.t()}
  def open_url(url, find \\ &System.find_executable/1) do
    unless url =~ ~r{\Ahttps?://} do
      raise ArgumentError, "open takes an http(s) URL, got: #{inspect(url)}"
    end

    case find.("ixterm") do
      nil ->
        {:error, "no ixterm on PATH to open a pane with"}

      exe ->
        case Cmd.run(exe, ["open", url], stderr_to_stdout: true) do
          {_output, 0} ->
            :ok

          {output, status} ->
            {:error, "ixterm open exited #{status}: #{String.trim(output)}"}
        end
    end
  end

  # -- preflight ----------------------------------------------------------------

  defp ensure_app!(dir) do
    unless File.exists?(Path.join(dir, "package.json")) do
      raise ArgumentError, "#{dir} has no package.json; scaffold it with `mkapp` first"
    end
  end

  defp ensure_deps!(dir) do
    unless File.dir?(Path.join(dir, "node_modules")) do
      {out, code} = Cmd.run("npm", ["install"], cd: dir, stderr_to_stdout: true)

      if code != 0 do
        raise "npm install failed in #{dir} (exit #{code}):\n" <> out
      end
    end

    :ok
  end
end
