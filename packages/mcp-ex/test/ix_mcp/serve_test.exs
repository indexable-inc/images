defmodule IxMcp.ServeTest do
  use ExUnit.Case, async: true

  alias IxMcp.Serve
  alias IxMcp.Serve.State

  # Fake app dirs never collide across async tests; State is global.
  defp unique_dir do
    Path.join(System.tmp_dir!(), "ix-serve-test-#{System.unique_integer([:positive])}")
  end

  defp recording_runner(pid, results) do
    fn _dir, step ->
      send(pid, {:ran, step})
      Map.fetch!(results, step)
    end
  end

  describe "open_url_escape/1 (the ix#8185 byte contract)" do
    test "emits exactly ESC ]5522;open-url;<url> BEL" do
      assert Serve.open_url_escape("http://hydra:5173/") ==
               <<0x1B, ?], ?5, ?5, ?2, ?2, ?;>> <>
                 "open-url;http://hydra:5173/" <> <<0x07>>
    end

    test "accepts https" do
      assert Serve.open_url_escape("https://hydra:5173/") ==
               "\e]5522;open-url;https://hydra:5173/\a"
    end

    test "rejects non-http(s) schemes" do
      for url <- ["file:///etc/passwd", "javascript:alert(1)", "ftp://x/", "hydra:5173"] do
        assert_raise ArgumentError, ~r/http\(s\)/, fn -> Serve.open_url_escape(url) end
      end
    end
  end

  describe "check_and_promote/2 (the gate decision)" do
    test "a green check promotes and records the outcome" do
      dir = unique_dir()
      runner = recording_runner(self(), %{check: {"all clear", 0}, promote: {"", 0}})

      assert Serve.check_and_promote(dir, runner) == :promoted
      assert_received {:ran, :check}
      assert_received {:ran, :promote}

      assert %{last_check: :ok, last_check_output: "all clear", last_error: nil} =
               State.get(dir)

      State.delete(dir)
    end

    test "a red check never promotes and keeps the error readable" do
      dir = unique_dir()

      runner =
        recording_runner(self(), %{
          check: {"src/lib/store.svelte.ts:3 error TS2345", 1},
          promote: {"", 0}
        })

      assert Serve.check_and_promote(dir, runner) == :rejected
      assert_received {:ran, :check}
      refute_received {:ran, :promote}

      assert %{last_check: :failed, last_check_output: output, last_error: error} =
               State.get(dir)

      assert output =~ "TS2345"
      assert error =~ "exited 1"
      State.delete(dir)
    end

    test "a failed promote after a green check is recorded, not silent" do
      dir = unique_dir()

      runner =
        recording_runner(self(), %{check: {"ok", 0}, promote: {"rsync: no such dir", 23}})

      assert Serve.check_and_promote(dir, runner) == :promote_failed
      assert %{last_check: :ok, last_error: error} = State.get(dir)
      assert error =~ "promote exited 23"
      assert error =~ "rsync"
      State.delete(dir)
    end
  end

  describe "signature/1" do
    test "changes when a staging file changes and is stable otherwise" do
      dir = unique_dir()
      staging = Path.join(dir, "staging")
      File.mkdir_p!(Path.join(staging, "lib"))
      File.write!(Path.join(staging, "lib/a.ts"), "export const a = 1;\n")
      on_exit(fn -> File.rm_rf!(dir) end)

      sig = Serve.signature(dir)
      assert Serve.signature(dir) == sig

      File.write!(Path.join(staging, "lib/a.ts"), "export const a = 2;;;;;\n")
      assert Serve.signature(dir) != sig
    end
  end

  describe "parse_port/1" do
    test "finds the vite Local URL through ANSI colors, bold port included" do
      output =
        "  VITE v7.0.0  ready in 312 ms\n\n" <>
          "  \e[32m➜\e[39m  \e[1mLocal\e[22m:   " <>
          "\e[36mhttp://localhost:\e[1m5173\e[22m/\e[39m\n" <>
          "  \e[32m➜\e[39m  \e[1mNetwork\e[22m: \e[36mhttp://192.168.1.7:5173/\e[39m\n"

      assert Serve.parse_port(output) == {:ok, 5173}
    end

    test "reports :error while the URL has not printed yet" do
      assert Serve.parse_port("") == :error
      assert Serve.parse_port("VITE v7.0.0 starting...") == :error
    end
  end

  describe "status/1 and stop/1" do
    test "an unserved dir is :not_serving" do
      dir = unique_dir()
      assert Serve.status(dir) == {:error, :not_serving}
      assert Serve.stop(dir) == {:error, :not_serving}
    end
  end
end
