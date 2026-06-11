defmodule SymphonyElixir.SkillTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Skill

  # Minimal valid YAML frontmatter shared by all fixture skills.
  @frontmatter """
  ---
  description: Fixture skill for parser tests.
  tools: []
  ---
  """

  defp setup_skill_dir do
    dir = Path.join(System.tmp_dir!(), "skill_test_#{System.unique_integer([:positive])}")
    partials_dir = Path.join(dir, "_partials")
    File.mkdir_p!(dir)
    File.mkdir_p!(partials_dir)

    on_exit(fn -> File.rm_rf!(dir) end)

    {dir, partials_dir}
  end

  defp write_skill!(dir, name, body) do
    path = Path.join(dir, "#{name}.md")
    File.write!(path, @frontmatter <> body)
    path
  end

  defp write_partial!(partials_dir, name, body) do
    File.write!(Path.join(partials_dir, "#{name}.md"), body)
  end

  describe "model-agnostic frontmatter" do
    # The execution envelope (engine, model, effort, permissions) lives on the
    # workflow `.sym` agent node, not in skill frontmatter. The skill is the
    # Agent Skills shape: an optional description plus a tools allowlist, never
    # a model. The loader must not carry the removed codex envelope fields.
    test "a skill with only a body and empty tools loads; description defaults to nil" do
      {dir, _partials_dir} = setup_skill_dir()

      path = Path.join(dir, "node_skill.md")

      File.write!(path, """
      ---
      tools: []
      ---
      Body for a model-agnostic skill.
      """)

      assert {:ok, skill} = Skill.load(path)
      assert skill.description == nil
      assert skill.tools == []
      assert String.contains?(skill.body, "model-agnostic skill")
      refute Map.has_key?(skill, :codex_model)
      refute Map.has_key?(skill, :sandbox)
      refute Map.has_key?(skill, :approval_policy)
      refute Map.has_key?(skill, :reasoning_effort)
    end

    test "description and tools are read from frontmatter" do
      {dir, _partials_dir} = setup_skill_dir()

      path = Path.join(dir, "described_skill.md")

      File.write!(path, """
      ---
      description: Does a specific thing.
      tools: [linear_graphql]
      ---
      Body.
      """)

      assert {:ok, skill} = Skill.load(path)
      assert skill.description == "Does a specific thing."
      assert skill.tools == ["linear_graphql"]
    end
  end

  describe "expand_partials: self-referential partial" do
    # Regression guard for the prod outage described in the plan. Partial files
    # that document their own token name in a prose header (e.g. "any skill that
    # references `{{partial:graphite_sop}}` gets this content inlined") would
    # leave a residual token in the catalog body under the old single-pass
    # implementation. The fixpoint + seen-set must drop the self-reference so
    # the stored body is token-free.
    test "a partial whose body contains its own token loads cleanly" do
      {dir, partials_dir} = setup_skill_dir()

      write_partial!(partials_dir, "policy", """
      This file is referenced via `{{partial:policy}}`.
      Actual policy content here.
      """)

      path = write_skill!(dir, "my_skill", "Use this:\n{{partial:policy}}\nDone.\n")

      assert {:ok, skill} = Skill.load(path)
      refute String.contains?(skill.body, "{{partial:")
      assert String.contains?(skill.body, "Actual policy content here.")
    end
  end

  describe "expand_partials: nested partials" do
    # Partial A references partial B. The fixpoint loop expands A on the first
    # pass, which introduces {{partial:b}} into the body; the second pass
    # expands B. The final body must contain B's text and no residual tokens.
    test "partial A inlining partial B both expand into the final body" do
      {dir, partials_dir} = setup_skill_dir()

      write_partial!(partials_dir, "a", "Content from A.\n{{partial:b}}\n")
      write_partial!(partials_dir, "b", "Content from B.")

      path = write_skill!(dir, "nested_skill", "Start.\n{{partial:a}}\nEnd.\n")

      assert {:ok, skill} = Skill.load(path)
      refute String.contains?(skill.body, "{{partial:")
      assert String.contains?(skill.body, "Content from A.")
      assert String.contains?(skill.body, "Content from B.")
    end
  end

  describe "expand_partials: missing partial" do
    # A token whose partial file is genuinely absent must still be a hard load
    # error. The seen-set logic must not shadow this: only already-seen names
    # are dropped; an unseen name with no file on disk is an error.
    test "a reference to a nonexistent partial returns a missing_partial error" do
      {dir, _partials_dir} = setup_skill_dir()

      path = write_skill!(dir, "broken_skill", "{{partial:does_not_exist}}\n")

      assert {:error, {:missing_partial, "does_not_exist", _reason}} = Skill.load(path)
    end
  end

  describe "expand_partials: repeated include" do
    # A partial is a named shared contract; a skill body that references the
    # same partial twice inlines its content once. This keeps the catalog body
    # deterministic and is the "inline each named partial at most once" half of
    # the fixpoint behavior (the other half drops self-reference tokens).
    test "the same partial referenced twice is inlined once" do
      {dir, partials_dir} = setup_skill_dir()

      write_partial!(partials_dir, "contract", "SHARED-CONTRACT-TEXT")

      path =
        write_skill!(
          dir,
          "repeat_skill",
          "First:\n{{partial:contract}}\nSecond:\n{{partial:contract}}\n"
        )

      assert {:ok, skill} = Skill.load(path)
      refute String.contains?(skill.body, "{{partial:")

      occurrences = skill.body |> String.split("SHARED-CONTRACT-TEXT") |> length() |> Kernel.-(1)
      assert occurrences == 1
    end
  end
end
