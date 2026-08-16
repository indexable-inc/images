defmodule IxMcp.LM.Backend do
  @moduledoc """
  What `IxMcp.LM` needs from a model provider, and nothing else.

  One callback, no streaming, no tools: a v0 sub-call is a question and an
  answer. That is what makes `IxMcp.LM.Stub` a real substitute rather than a
  pretend one, so the memoization, budget and event-log behaviour are all
  testable without a network or a key.

  Selected by `config :ix_mcp, :lm_backend, MyBackend`; the default is
  `IxMcp.LM.Anthropic`.
  """

  @typedoc "A sub-call, already assembled: context is inside `prompt`."
  @type request :: %{
          required(:model) => String.t(),
          required(:prompt) => String.t(),
          required(:max_tokens) => pos_integer(),
          optional(:system) => String.t() | nil
        }

  @typedoc """
  An answer, with the token counts the budget meter charges against.
  `tokens_in` must be the WHOLE prompt cost, cache reads and writes
  included.
  """
  @type response :: %{
          required(:text) => String.t(),
          required(:tokens_in) => non_neg_integer(),
          required(:tokens_out) => non_neg_integer(),
          required(:model) => String.t(),
          optional(:stop_reason) => String.t() | nil
        }

  @callback complete(request()) :: {:ok, response()} | {:error, term()}
end
