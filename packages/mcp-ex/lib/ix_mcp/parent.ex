defmodule IxMcp.Parent do
  @moduledoc """
  The child half of the subagent channel: how a spawned agent's kernel speaks
  to the kernel that spawned it, before it has a final answer.

  Parent to child already existed (`IxMcp.Agents.send/2`, delivered as a user
  turn after the child's next tool result). This is the return path. A child
  used to have exactly one outbound message -- its final response -- so a
  half-finished finding, a blocked dependency, or a question only the parent
  could answer had nowhere to go but the end of the run, by which time the
  parent had usually moved on.

  Transport is the existing cross-session bus rather than a new one. Parent and
  child kernels are separate OS processes on one host sharing one actions.db,
  already the claim arbiter and the message bus (#3880, #3881), and the parent's
  `IxMcp.SessionWatch` sweep delivers a row as a channel notification within a
  few seconds. Nothing here reaches beyond the parent: a child holds its
  parent's session id and no sibling's, which is what keeps the tree's messaging
  a star at every level instead of a mesh.

  Only a spawned child has a parent. `IxMcp.Agents.Backend` sets
  `IX_AGENT_PARENT_SESSION` and `IX_AGENT_ID` when it builds a child's kernel,
  and every function here raises without them rather than guessing a target.
  """

  alias IxMcp.Sessions

  @doc """
  Message the parent session. The text arrives tagged with this agent's id, so
  a parent fanned out over several children can tell them apart.
  """
  @spec send(String.t()) :: {:ok, String.t()} | {:error, String.t()}
  def send(text) when is_binary(text) do
    Sessions.send(parent_session!(), "[subagent #{id!()}] " <> text)
  end

  @doc "True in a kernel that `IxMcp.Agents` spawned."
  @spec child?() :: boolean()
  def child?, do: System.get_env("IX_AGENT_PARENT_SESSION") != nil

  @doc "The spawning session's directory id."
  @spec parent_session!() :: integer()
  def parent_session! do
    value =
      System.get_env("IX_AGENT_PARENT_SESSION") ||
        raise "no parent: IX_AGENT_PARENT_SESSION is unset, so this kernel is a lead. " <>
                "Use Sessions.send/2 to message a peer session."

    case Integer.parse(value) do
      {id, ""} -> id
      _junk -> raise "IX_AGENT_PARENT_SESSION is #{inspect(value)}, not a session id"
    end
  end

  @doc "This agent's id in its parent's roster."
  @spec id!() :: String.t()
  def id! do
    System.get_env("IX_AGENT_ID") ||
      raise "no agent id: IX_AGENT_ID is unset, so this kernel is a lead"
  end
end
