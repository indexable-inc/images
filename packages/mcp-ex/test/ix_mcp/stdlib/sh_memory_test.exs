defmodule IxMcp.Stdlib.ShMemoryTest do
  # async: false ON PURPOSE. :erlang.memory(:binary) is a VM-WIDE counter, so a
  # neighbouring async test allocating binaries makes this assertion say whatever
  # the schedule says. A measurement whose instrument is shared is not a
  # measurement.
  use ExUnit.Case, async: false

  alias IxMcp.Stdlib.Sh

  @cap 16_384
  @stderr_bytes 33_554_432

  @tag :tmp_dir
  test "a huge stderr is capped by SEEKING, not by reading it all in", %{tmp_dir: dir} do
    # Reading the file and capping afterwards cost 256 MB of binary memory to
    # KEEP 16 KiB, per stage, in the module whose docs advertise a cap -- so a
    # runaway loop's diagnostics could OOM the node.
    :erlang.garbage_collect()
    before = :erlang.memory(:binary)

    result =
      Sh.run(
        Sh.cmd(["sh", "-c", "head -c #{@stderr_bytes} /dev/zero | tr '\\0' 'x' 1>&2"]),
        timeout_ms: 120_000,
        scratch_root: dir
      )

    [stage] = result.stages

    assert stage.stderr_bytes == @stderr_bytes
    assert stage.stderr_truncated
    assert byte_size(stage.stderr) <= @cap

    # 32 MB of stderr must not become 32 MB of binary heap. The bound is generous
    # (a quarter of the file) so it cannot flake on allocator slack, and it is
    # still two orders of magnitude below the read-it-all behaviour.
    growth = :erlang.memory(:binary) - before
    assert growth < div(@stderr_bytes, 4), "binary memory grew by #{growth} bytes"
  end
end
