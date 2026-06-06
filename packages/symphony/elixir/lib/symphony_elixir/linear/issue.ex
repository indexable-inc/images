defmodule SymphonyElixir.Linear.Issue do
  @moduledoc """
  Minimal Linear issue representation used by the orchestrator.
  Carries only what the trigger needs to enqueue and identify a run.
  """

  @enforce_keys [:id, :identifier, :labels]
  defstruct [:id, :identifier, :title, :url, :state, :labels]

  @type t :: %__MODULE__{
          id: String.t(),
          identifier: String.t(),
          title: String.t() | nil,
          url: String.t() | nil,
          state: String.t() | nil,
          labels: [String.t()]
        }
end
