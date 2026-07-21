defmodule IxMcp.Ask do
  @moduledoc """
  Put a question to the human through the client's native dialog, via MCP
  elicitation (`elicitation/create`). The successor to Claude Code's built-in
  AskUserQuestion tool (index#3856): a cell calls `Ask.user/2`, the client
  renders the dialog, and the cell gets the answer back as a value.

  The call blocks its cell (never the server: each cell runs in its own
  process), so under `exec`'s budget-then-background contract a pending
  question simply becomes a background job whose finish notification carries
  the answer.

      Ask.user("Ship the redesign or patch the old shape?",
        options: ["Redesign", {"Patch", "keep the current structure"}])
      #=> {:ok, "Redesign"} | :declined | :cancelled | :timeout

  With no `options` the dialog takes free text. There is no capability
  sniffing: a client that cannot elicit answers with a JSON-RPC error, which
  raises here with that error in hand.
  """

  alias IxMcp.MCP.ClientRequests

  @default_timeout_ms :timer.minutes(30)

  @typedoc "A choice: the value itself, or {value, description}."
  @type option :: String.t() | {String.t(), String.t()}

  @doc """
  Ask the user one question and wait for the answer.

  Options:
    * `:options` - list of choices rendered as a select; omit for free text.
    * `:timeout_ms` - how long to wait before giving up (default 30 min);
      on expiry the dialog is cancelled and `:timeout` is returned, so an
      unattended run parks instead of hanging forever.
  """
  @spec user(String.t(), [{:options, [option()]} | {:timeout_ms, pos_integer()}]) ::
          {:ok, String.t()} | :declined | :cancelled | :timeout
  def user(message, opts \\ []) when is_binary(message) and is_list(opts) do
    params = %{
      "message" => message,
      "requestedSchema" => schema(Keyword.get(opts, :options))
    }

    case ClientRequests.request(
           "elicitation/create",
           params,
           Keyword.get(opts, :timeout_ms, @default_timeout_ms)
         ) do
      {:ok, %{"action" => "accept", "content" => %{"answer" => answer}}} when is_binary(answer) ->
        {:ok, answer}

      {:ok, %{"action" => "decline"}} ->
        :declined

      {:ok, %{"action" => "cancel"}} ->
        :cancelled

      {:ok, other} ->
        raise "elicitation returned an unexpected result: " <> inspect(other)

      {:error, :timeout} ->
        :timeout

      {:error, :no_transport} ->
        raise "no MCP client is connected; Ask.user needs a live Claude Code session"

      {:error, :transport_closed} ->
        raise "the MCP client disconnected while the question was pending"

      {:error, {:client_error, error}} ->
        raise "the client rejected the elicitation request: " <> inspect(error)
    end
  end

  # MCP elicitation schemas are flat objects of primitives; one string field
  # named `answer` is the whole surface. Options become an enum, which the
  # client renders as a select; enumNames carry the descriptions.
  defp schema(nil) do
    %{
      "type" => "object",
      "properties" => %{"answer" => %{"type" => "string", "title" => "Answer"}},
      "required" => ["answer"]
    }
  end

  defp schema([_ | _] = options) do
    %{
      "type" => "object",
      "properties" => %{
        "answer" => %{
          "type" => "string",
          "title" => "Answer",
          "enum" => Enum.map(options, &value_of/1),
          "enumNames" => Enum.map(options, &name_of/1)
        }
      },
      "required" => ["answer"]
    }
  end

  defp value_of({value, _description}) when is_binary(value), do: value
  defp value_of(value) when is_binary(value), do: value

  defp name_of({value, description}) when is_binary(value) and is_binary(description),
    do: value <> ": " <> description

  defp name_of(value) when is_binary(value), do: value
end
