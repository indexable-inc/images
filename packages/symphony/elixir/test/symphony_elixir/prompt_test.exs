defmodule SymphonyElixir.PromptTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Prompt

  describe "build/2 inline" do
    test "returns inline text verbatim" do
      assert {:ok, "do the thing"} = Prompt.build({:inline, "do the thing"})
    end

    test "an unresolved inline prompt (nil text) is an error" do
      assert {:error, :unresolved_inline_prompt} = Prompt.build({:inline, nil})
    end
  end

  describe "build/2 skill" do
    test "loads the body through the resolver and interpolates bindings" do
      resolver = fn "inspect" -> {:ok, "Inspect ${repo} on branch ${branch}."} end

      assert {:ok, "Inspect symphony on branch main."} =
               Prompt.build({:skill, "inspect", %{"repo" => "symphony", "branch" => "main"}}, resolver: resolver)
    end

    test "reads a dotted binding path" do
      resolver = fn _ -> {:ok, "Ticket ${ticket.id}: ${ticket.title}"} end
      bindings = %{"ticket" => %{"id" => "ABC-1", "title" => "Fix it"}}

      assert {:ok, "Ticket ABC-1: Fix it"} = Prompt.build({:skill, "impl", bindings}, resolver: resolver)
    end

    test "a placeholder with no binding fails loudly" do
      resolver = fn _ -> {:ok, "needs ${missing}"} end
      assert {:error, {:unbound_placeholder, "missing"}} = Prompt.build({:skill, "x", %{}}, resolver: resolver)
    end

    test "a skill ref with no resolver is an error" do
      assert {:error, :missing_skill_resolver} = Prompt.build({:skill, "x", %{}})
    end

    test "propagates a resolver failure" do
      resolver = fn _ -> {:error, :enoent} end
      assert {:error, :enoent} = Prompt.build({:skill, "missing", %{}}, resolver: resolver)
    end

    test "expands {{partial:name}} includes through the partial resolver" do
      resolver = fn _ -> {:ok, "Start.\n{{partial:pr}}\nEnd ${who}."} end
      partial_resolver = fn "pr" -> {:ok, "Open a PR."} end

      assert {:ok, rendered} =
               Prompt.build({:skill, "impl", %{"who" => "you"}},
                 resolver: resolver,
                 partial_resolver: partial_resolver
               )

      assert rendered == "Start.\nOpen a PR.\nEnd you."
    end

    test "a body that references a partial with no partial resolver fails" do
      resolver = fn _ -> {:ok, "{{partial:pr}}"} end
      assert {:error, {:missing_partial_resolver, ["pr"]}} = Prompt.build({:skill, "x", %{}}, resolver: resolver)
    end
  end

  describe "render/2" do
    test "leaves a bare dollar sign untouched" do
      assert {:ok, "cost is $5 and ${x}"} = Prompt.render("cost is $5 and ${x}", %{"x" => "${x}"})
    end

    test "stringifies non-string bindings" do
      assert {:ok, "count 3"} = Prompt.render("count ${n}", %{"n" => 3})
    end

    test "an escaped $${path} renders a literal ${path} with no binding" do
      assert {:ok, "?pub_secret=${pub_secret}"} = Prompt.render("?pub_secret=$${pub_secret}", %{})
    end

    test "an escape and a real placeholder coexist in one body" do
      assert {:ok, "url=${pub_secret} repo=symphony"} =
               Prompt.render("url=$${pub_secret} repo=${repo}", %{"repo" => "symphony"})
    end

    test "an unescaped placeholder still fails loudly when an escape is present" do
      assert {:error, {:unbound_placeholder, "missing"}} =
               Prompt.render("keep $${literal} but ${missing}", %{})
    end
  end

  describe "build/2 escape" do
    test "a skill body with a shell $${var} reaches the engine as a literal" do
      resolver = fn _ -> {:ok, "curl ...?pub_secret=$${pub_secret}"} end
      assert {:ok, "curl ...?pub_secret=${pub_secret}"} = Prompt.build({:skill, "focus_route", %{}}, resolver: resolver)
    end
  end
end
