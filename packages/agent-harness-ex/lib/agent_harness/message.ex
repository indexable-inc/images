defmodule AgentHarness.Message do
  @moduledoc """
  The wire shape of everything agents exchange.

  Three kinds ride the same struct:

    * `:message` - an explicit Send Message between any two agents.
    * `:final` - a subagent's final response, routed to the lead when its
      runner returns `{:ok, text}` (the card: "a subagent's final response
      is delivered to the lead as a message").
    * `:error` - the runner returned an error or crashed; `text` carries the
      inspected reason so the lead can decide whether to re-instruct.
  """

  @enforce_keys [:from, :to, :text, :kind, :sent_at_ms]
  defstruct [:from, :to, :text, :kind, :sent_at_ms]

  @type kind :: :message | :final | :error

  @type t :: %__MODULE__{
          from: String.t(),
          to: String.t(),
          text: String.t(),
          kind: kind(),
          sent_at_ms: integer()
        }

  @spec new(String.t(), String.t(), String.t(), kind()) :: t()
  def new(from, to, text, kind)
      when is_binary(from) and is_binary(to) and is_binary(text) do
    %__MODULE__{
      from: from,
      to: to,
      text: text,
      kind: kind,
      sent_at_ms: System.system_time(:millisecond)
    }
  end
end
