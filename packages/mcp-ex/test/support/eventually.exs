# Shared ExUnit support, loaded from test_helper.exs. Several suites wait on
# state that only becomes true asynchronously -- a supervisor restart, an
# outbox row the notifier has yet to write, a viewer registration crossing the
# NIF -- so they poll for it with a grace window instead of sleeping a fixed
# amount and hoping.
defmodule IxMcpTest.Eventually do
  @moduledoc false

  import ExUnit.Assertions, only: [flunk: 1]

  @doc """
  Polls `probe` every 20ms until it answers with something other than `nil`,
  and returns that value.

  Exhausting `tries` fails the calling test rather than returning `nil`, so a
  caller can never assert against a timeout it did not notice.
  """
  def eventually(probe, tries \\ 50) do
    case probe.() do
      nil when tries > 0 ->
        Process.sleep(20)
        eventually(probe, tries - 1)

      nil ->
        flunk("condition never became true")

      value ->
        value
    end
  end
end
