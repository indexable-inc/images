defmodule IxMcp.MCP.SurfaceGuideTest do
  use ExUnit.Case, async: true

  alias IxMcp.MCP.Tools

  # The guide is read twice: server.ex splices it into `instructions` (delivered
  # on connect, so every MCP client sees it) and the exec tool description
  # carries it. Both readers truncate around 50 lines, so the guide's line ORDER
  # is a user-visible contract rather than a matter of taste.
  #
  # THE CAPABILITY CEILINGS ARE NOT HERE. mcp_test.exs already owns them, in
  # "the exec guide names the fan-out surface inside the client's truncation
  # budget": Agents.spawn( below guide index 40, Cmd.run( and Edit.replace(
  # below 50, each also asserted to EXIST so a vanished marker fails too. Two
  # gates freezing the same numbers is how the two numbers drift apart, so this
  # file deliberately asserts nothing about those markers.
  #
  # What it does cover is the part that budget test cannot: a future edit could
  # push THIS lane's block down while Agents.spawn/Cmd.run/Edit.replace all stay
  # comfortably inside their ceilings, and the verdict a reader is supposed to
  # meet first would quietly slide past the cut with the gate still green.
  #
  # Units, since a bare index invites the wrong denominator: every number below
  # is a 0-based line index within `Tools.surface_guide/0`'s own text -- the same
  # unit mcp_test.exs uses -- not a source line of tools.ex and not a line of the
  # rendered instructions (in the exec description the guide starts about 19
  # lines further down).
  #
  # Measured against main 2180f3d1f671 with this lane applied: verdict at 0, the five
  # Sh example lines at 9-13, guide length 245 (main's own 246).
  @sh_markers ["Sh.pipeline(", "Sh.ok", "Sh.mutate", "Sh.watch", "Sh.run("]
  @verdict "STRONGLY PREFER exec"
  @window 40

  defp guide_lines, do: Tools.surface_guide() |> String.split("\n")

  defp index_of(needle), do: Enum.find_index(guide_lines(), &String.contains?(&1, needle))

  test "the anti-Bash verdict OPENS the guide, where truncation cannot reach it" do
    index = index_of(@verdict)

    assert is_integer(index),
           "the verdict #{inspect(@verdict)} is gone from the guide: the whole point of the " <>
             "block is that a reader who stops at the truncation still learns exec is preferred."

    assert index == 0,
           "the verdict is at guide index #{index}, not 0. It is meant to OPEN the guide, and " <>
             "`<= 4` gave four lines of slack to an assertion whose own name says index 0: a " <>
             "one-to-four-line push was the mutant this gate was listed as catching."
  end

  test "a runnable Sh example rides inside the reader's window, not below the cut" do
    window = guide_lines() |> Enum.take(@window) |> Enum.join("\n")

    for marker <- @sh_markers do
      assert String.contains?(window, marker),
             "#{marker} is not in the guide's first #{@window} lines. An example below the cut " <>
               "teaches nobody; compress something above it rather than appending."
    end
  end

  test "the verdict precedes every Sh example line, so the reason arrives before the syntax" do
    verdict = index_of(@verdict)

    for marker <- @sh_markers do
      # `index = index_of(marker)` as a comprehension clause is a FILTER, not a
      # binding: a marker that had vanished from the guide made index_of return
      # nil, which skipped the iteration silently and left this gate asserting
      # nothing at all. Bind first, then assert the binding is real.
      index = index_of(marker)

      assert is_integer(index),
             "#{marker} is gone from the guide entirely, so the ordering gate had nothing to check."

      assert verdict < index,
             "#{marker} appears at guide index #{index}, above the verdict at #{verdict}."
    end
  end
end
