defmodule IxMcp.CredentialTest do
  use ExUnit.Case, async: false

  alias IxMcp.Credential

  @moduletag :tmp_dir

  setup do
    on_exit(fn -> System.delete_env("IX_CREDENTIAL_SOCK") end)
    :ok
  end

  describe "socket_path/0" do
    test "an explicit override wins over the derived path" do
      System.put_env("IX_CREDENTIAL_SOCK", "/tmp/explicit.sock")
      assert Credential.socket_path() == "/tmp/explicit.sock"
    end

    test "the derived path is per-uid, so two operators on one host do not collide" do
      System.delete_env("IX_CREDENTIAL_SOCK")
      path = Credential.socket_path()

      {uid, _} = System.cmd("id", ["-u"])
      assert String.contains?(path, String.trim(uid))
      assert String.ends_with?(path, ".sock")
    end

    # The derivation is written twice, here and in
    # packages/ix-credential/src/socket.rs, because a subprocess on every
    # lookup is the wrong trade for a three-line rule. This is what makes
    # the second copy safe: the two disagree loudly rather than silently.
    @tag :ix_credential_binary
    test "the kernel and the binary derive the same path" do
      System.delete_env("IX_CREDENTIAL_SOCK")

      case System.find_executable("ix-credential") do
        nil ->
          # Nothing to compare against; the nix check provides the binary.
          assert Credential.socket_path() != ""

        binary ->
          {out, 0} = System.cmd(binary, ["socket-path"])
          assert String.trim(out) == Credential.socket_path()
      end
    end
  end

  describe "status/0" do
    test "an absent socket says no loan is running, and how to start one", %{tmp_dir: dir} do
      System.put_env("IX_CREDENTIAL_SOCK", Path.join(dir, "absent.sock"))

      assert {:error, message} = Credential.status()
      assert message =~ "no credential loan is running"
      assert message =~ "ix-credential lend"
    end

    test "an ordinary file on the path is a different message from an absent one", %{
      tmp_dir: dir
    } do
      path = Path.join(dir, "not-a-socket")
      File.write!(path, "")
      System.put_env("IX_CREDENTIAL_SOCK", path)

      assert {:error, message} = Credential.status()
      assert message =~ "is not a socket"
      refute message =~ "does not exist"
    end

    test "a bound socket with no helper on PATH names the missing helper" do
      # Not the ExUnit tmp_dir: a unix socket path is capped at 104 bytes on
      # darwin, and tmp_dir embeds the module and the test name, which puts
      # it well past that. The real path, /run/ix-credential/<uid>.sock, has
      # the same budget and a great deal more room in it.
      path = short_socket_path()
      listener = listen!(path)

      System.put_env("IX_CREDENTIAL_SOCK", path)

      case System.find_executable("ix-credential") do
        nil ->
          assert {:error, message} = Credential.status()
          assert message =~ "not on PATH"

        _present ->
          assert {:ok, ^path} = Credential.status()
      end

      :gen_tcp.close(listener)
      File.rm(path)
    end
  end

  describe "run/3" do
    test "refuses to run at all when no loan is live", %{tmp_dir: dir} do
      System.put_env("IX_CREDENTIAL_SOCK", Path.join(dir, "absent.sock"))

      # The point of raising: a build that silently runs without a
      # credential fails later, with github's message instead of ours.
      assert_raise RuntimeError, ~r/no credential loan is running/, fn ->
        Credential.run("git", ["ls-remote", "https://github.com/indexable-inc/nox"])
      end
    end
  end

  describe "credential_env/2" do
    test "wires git to the helper and points it at the socket" do
      env = Credential.credential_env("/run/ix-credential/0.sock")

      assert {"IX_CREDENTIAL_SOCK", "/run/ix-credential/0.sock"} in env
      assert {"GIT_CONFIG_KEY_0", "credential.helper"} in env
      assert {"GIT_CONFIG_COUNT", "1"} in env
      assert {"GIT_TERMINAL_PROMPT", "0"} in env

      {_key, value} = List.keyfind(env, "GIT_CONFIG_VALUE_0", 0)
      # git reads a value with a space as a command line, which is how the
      # subcommand rides along with the absolute path.
      assert String.ends_with?(value, " helper")
    end

    test "appends to a caller's git config instead of clobbering it" do
      caller = [
        {"GIT_CONFIG_COUNT", "2"},
        {"GIT_CONFIG_KEY_0", "user.name"},
        {"GIT_CONFIG_VALUE_0", "someone"},
        {"GIT_CONFIG_KEY_1", "user.email"},
        {"GIT_CONFIG_VALUE_1", "someone@example.com"}
      ]

      env = Credential.credential_env("/run/ix-credential/0.sock", env: caller)

      # The caller's two entries survive untouched.
      assert {"GIT_CONFIG_KEY_0", "user.name"} in env
      assert {"GIT_CONFIG_KEY_1", "user.email"} in env
      # Ours lands at the next free index, and the count covers all three.
      assert {"GIT_CONFIG_KEY_2", "credential.helper"} in env
      assert List.keyfind(env, "GIT_CONFIG_COUNT", 0) == {"GIT_CONFIG_COUNT", "3"}
    end

    test "a caller's unrelated environment is preserved" do
      env = Credential.credential_env("/x.sock", env: [{"FOO", "bar"}])
      assert {"FOO", "bar"} in env
    end
  end

  # `{:local, path}` needs an absolute path and binds a real unix socket, so
  # `status/0` sees the same inode type sshd's forward creates.
  defp listen!(path) do
    {:ok, listener} = :gen_tcp.listen(0, [{:ifaddr, {:local, path}}, :binary, {:active, false}])
    listener
  end

  defp short_socket_path do
    suffix = Base.encode16(:crypto.strong_rand_bytes(4), case: :lower)
    Path.join(System.tmp_dir!(), "ixc-#{suffix}.sock")
  end
end
