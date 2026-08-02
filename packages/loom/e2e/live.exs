# Rung-2 live e2e: the full loom lifecycle against the real ix API,
# run from INSIDE the control VM (self-fork). See README "Testing".
#
#   MIX_ENV=prod mix run --no-deps-check e2e/live.exs
#
# Required env: LOOM_PARENT_VM (the VM to fork - the control VM itself),
# LOOM_IX_BIN, LOOM_CLAUDE_BIN (wrapper exporting ANTHROPIC_API_KEY).
# Prints one timestamped line per lifecycle event; exits 0 only when the
# whole spawn -> final -> stop -> wake -> final -> stop -> delete loop
# completed.

defmodule LoomE2E do
  @spec log(String.t()) :: :ok
  def log(message) do
    IO.puts("#{DateTime.to_iso8601(DateTime.utc_now())} #{message}")
  end

  @spec expect(String.t(), String.t(), integer(), timeout()) :: term()
  def expect(id, label, t0, timeout) do
    receive do
      {:loom, ^id, event} ->
        elapsed = System.monotonic_time(:millisecond) - t0
        log("+#{elapsed}ms #{label}: #{inspect(event)}")
        event
    after
      timeout ->
        log("TIMEOUT waiting for #{label}")
        System.halt(2)
    end
  end
end

parent = System.fetch_env!("LOOM_PARENT_VM")
Application.put_env(:loom, :ix_bin, System.get_env("LOOM_IX_BIN", "ix"))
Application.put_env(:loom, :claude_bin, System.get_env("LOOM_CLAUDE_BIN", "claude"))

# Gate the child on the fork's interior actually being ready: the
# secret file materialized and the claude binary reachable.
Application.put_env(
  :loom,
  :preflight,
  System.get_env(
    "LOOM_PREFLIGHT",
    "test -s /var/lib/loom/anthropic_api_key && test -x /root/bin/claude"
  )
)

# Same-node hairpin workaround (see Loom.Ix): pin fork placement.
case System.get_env("LOOM_RESTORE_ARGS") do
  nil -> :ok
  args -> Application.put_env(:loom, :restore_args, String.split(args))
end

case System.get_env("LOOM_IX_PREFIX") do
  nil -> :ok
  args -> Application.put_env(:loom, :ix_prefix, String.split(args))
end

{:ok, _apps} = Application.ensure_all_started(:loom)

LoomE2E.log("baseline identity: #{Loom.Guard.baseline() |> String.slice(0, 60)}")

t0 = System.monotonic_time(:millisecond)

{:ok, id} =
  Loom.spawn(
    "Reply with exactly LOOM-E2E-CHILD-OK and nothing else. Do not use any tools.",
    parent_vm: parent
  )

LoomE2E.log("spawn requested id=#{id} (fork of #{parent})")

{:spawned, vm} = LoomE2E.expect(id, "fork created", t0, 900_000)
LoomE2E.log("fork vm: #{vm}")

case LoomE2E.expect(id, "child finished", t0, 900_000) do
  {:final, text} ->
    if text =~ "LOOM-E2E-CHILD-OK",
      do: LoomE2E.log("CHILD RESULT OK"),
      else: LoomE2E.log("CHILD RESULT UNEXPECTED: #{inspect(text)}")

  other ->
    LoomE2E.log("child failed: #{inspect(other)}")
    System.halt(3)
end

LoomE2E.expect(id, "fork stopped", t0, 900_000)

{:ok, status} = Loom.status(id)
LoomE2E.log("status after turn 1: #{inspect(Map.take(status, [:phase, :session_id]))}")

t1 = System.monotonic_time(:millisecond)
:ok = Loom.send_text(id, "Reply with exactly LOOM-E2E-WAKE-OK and nothing else.")
LoomE2E.log("wake requested")

LoomE2E.expect(id, "woken", t1, 900_000)

case LoomE2E.expect(id, "wake turn finished", t1, 900_000) do
  {:final, text} ->
    if text =~ "LOOM-E2E-WAKE-OK",
      do: LoomE2E.log("WAKE RESULT OK"),
      else: LoomE2E.log("WAKE RESULT UNEXPECTED: #{inspect(text)}")

  other ->
    LoomE2E.log("wake failed: #{inspect(other)}")
    System.halt(4)
end

LoomE2E.expect(id, "fork stopped again", t1, 900_000)

:ok = Loom.delete(id)
LoomE2E.log("fork deleted; E2E COMPLETE")
