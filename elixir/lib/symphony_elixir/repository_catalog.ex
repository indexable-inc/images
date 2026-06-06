defmodule SymphonyElixir.RepositoryCatalog do
  @moduledoc """
  The repositories each Symphony skill workspace receives.

  Repository membership lives in the selected workflow pack's
  \`repositories.yaml\`. Every entry is cloned for each skill run, with writable
  refs and a run-scoped branch, so agents can open PRs in any listed repository
  when the skill asks them to.
  """

  alias SymphonyElixir.Config

  defstruct [:name, :owner_repo, :default_branch, :primary?]

  @type t :: %__MODULE__{
          name: String.t(),
          owner_repo: String.t(),
          default_branch: String.t(),
          primary?: boolean()
        }

  @spec all(Config.t()) :: [t()]
  def all(%Config{} = config) do
    config.repositories_file
    |> read_yaml!()
    |> Map.fetch!("repositories")
    |> Enum.map(&repo_from_map!/1)
  end

  @spec primary(Config.t()) :: t()
  def primary(%Config{} = config) do
    repos = all(config)
    primaries = Enum.filter(repos, & &1.primary?)

    case primaries do
      [repo] -> repo
      [] -> raise "RepositoryCatalog must define one primary repo"
      _ -> raise "RepositoryCatalog must define exactly one primary repo"
    end
  end

  defp repo_from_map!(%{} = map) do
    %__MODULE__{
      name: fetch_string!(map, "name"),
      owner_repo: fetch_string!(map, "owner_repo"),
      default_branch: fetch_string!(map, "default_branch"),
      primary?: Map.get(map, "primary", false) == true
    }
  end

  defp fetch_string!(map, key) do
    case Map.fetch!(map, key) do
      value when is_binary(value) and value != "" -> value
      value -> raise "repositories.yaml field #{key} must be a non-empty string, got #{inspect(value)}"
    end
  end

  defp read_yaml!(path) do
    case path |> File.read!() |> YamlElixir.read_from_string() do
      {:ok, decoded} when is_map(decoded) -> decoded
      {:ok, other} -> raise "repositories.yaml must decode to a map, got #{inspect(other)}"
      {:error, reason} -> raise "failed to read repositories.yaml: #{inspect(reason)}"
    end
  end
end
