defmodule IxMcp.Mac do
  @moduledoc """
  Where a mac-only side effect runs: this machine, or a macOS guest VM
  running its own BEAM node.

  `Imsg` and friends drive Messages.app through `osascript`, which only
  works on the mac whose user is signed in. The kernel's own mac is signed
  in as the user; a guest VM is signed in as the agent's own Apple ID. Both
  are reachable the same way once the guest runs a node, so the choice of
  machine is a value threaded through the caller rather than a second copy
  of the send logic.

      Mac.local() |> Mac.cmd("/usr/bin/osascript", ["-e", script])
      Mac.guest(:"ixagent@192.168.64.6") |> Mac.cmd(...)

  A guest call is an `:erpc` to a node holding the privacy grants
  (Full Disk Access, Automation) that let it read Messages and script it.
  Grants attach to the executable that runs, so the guest node's path is
  fixed by the launchd agent that starts it.
  """

  @typedoc "Local machine, or a named BEAM node on a macOS guest."
  @opaque t :: :local | {:node, node()}

  @call_timeout_ms 30_000

  @doc "The mac the kernel itself runs on."
  @spec local() :: t()
  def local, do: :local

  @doc "A macOS guest VM by the node name its agent registers."
  @spec guest(node()) :: t()
  def guest(node) when is_atom(node), do: {:node, node}

  @doc """
  Read `opts[:mac]`, defaulting to this machine, so every caller takes the
  same option without repeating the default.
  """
  @spec from_opts(keyword()) :: t()
  def from_opts(opts), do: Keyword.get(opts, :mac, :local)

  @doc "Run `cmd` with `args`, capturing stdout and stderr together."
  @spec cmd(t(), Path.t(), [String.t()]) :: {:ok, String.t()} | {:error, String.t()}
  def cmd(mac, cmd, args) do
    case apply_on(mac, System, :cmd, [cmd, args, [stderr_to_stdout: true]]) do
      {:ok, {out, 0}} -> {:ok, out}
      {:ok, {out, code}} -> {:error, "#{Path.basename(cmd)} exit #{code}: #{String.trim(out)}"}
      {:error, reason} -> {:error, reason}
    end
  end

  @doc "Whether `path` exists on that mac."
  @spec exists?(t(), Path.t()) :: boolean()
  def exists?(mac, path) do
    match?({:ok, true}, apply_on(mac, File, :exists?, [path]))
  end

  @doc "Whether this is the mac the kernel runs on."
  @spec local?(t()) :: boolean()
  def local?(:local), do: true
  def local?({:node, _}), do: false

  @doc "Human name for that mac, for error messages."
  @spec describe(t()) :: String.t()
  def describe(:local), do: "this mac"
  def describe({:node, node}), do: "guest #{node}"

  # A guest that is down or holding a different cookie is the common
  # failure and reads as a plain sentence, not an erpc stacktrace: the
  # node is a VM the caller can start again, not a bug in the caller.
  defp apply_on(:local, mod, fun, args), do: {:ok, apply(mod, fun, args)}

  defp apply_on({:node, node}, mod, fun, args) do
    {:ok, :erpc.call(node, mod, fun, args, @call_timeout_ms)}
  rescue
    e in ErlangError -> {:error, "#{node}: #{inspect(e.original)}"}
  catch
    :exit, reason -> {:error, "#{node} unreachable: #{inspect(reason)}"}
  end
end
