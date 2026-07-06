defmodule SymphonyElixir.IR.RunNotifierTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Config
  alias SymphonyElixir.IR.{Attempt, Node, RunGraph, RunNotifier}

  defp graph(attrs) do
    defaults = %{run_id: "triage-1780166452589-58", source_hash: "hash", status: :succeeded, nodes: %{}}
    struct(RunGraph, Map.merge(defaults, Map.new(attrs)))
  end

  # A succeeded agent node carrying one attempt with the given room-server
  # thread id, so the run-details link can resolve a deep link.
  defp agent_node(id, thread_id) do
    %Node{
      id: id,
      ast_origin: {:agent, id},
      kind: :agent,
      inputs: [],
      deps: [],
      state: :succeeded,
      attempts: [%Attempt{n: 1, engine: :codex, thread_id: thread_id, state: :succeeded, started_at: ~U[2026-06-04 00:00:00Z]}]
    }
  end

  # An exec node whose (possibly structured) output the content sections
  # read; `deps` is hand-set so a test can shape sink vs interior directly.
  defp exec_node(id, output, deps \\ []) do
    %Node{
      id: id,
      ast_origin: {:exec, id},
      kind: :exec,
      inputs: [],
      deps: deps,
      state: :succeeded,
      output: output
    }
  end

  # The notifier only reads the two cron-policy fields; default to the
  # production defaults (failures on, no success allowlist) unless overridden.
  defp config(attrs \\ %{}) do
    defaults = %{slack_notify_cron_failures: true, slack_notify_cron_workflows: []}
    struct(Config, Map.merge(defaults, Map.new(attrs)))
  end

  describe "notify?/2" do
    test "skips non-terminal runs" do
      refute RunNotifier.notify?(graph(status: :running, trigger: %{kind: :linear}), config())
      refute RunNotifier.notify?(graph(status: :pending, trigger: %{kind: :linear}), config())
    end

    test "skips cancelled runs" do
      refute RunNotifier.notify?(graph(status: :cancelled, trigger: %{kind: :linear}), config())
    end

    test "notifies on terminal non-cron runs" do
      assert RunNotifier.notify?(graph(status: :succeeded, trigger: %{kind: :linear}), config())
      assert RunNotifier.notify?(graph(status: :failed, trigger: %{kind: :manual}), config())
      # Absent trigger is not cron, so it notifies.
      assert RunNotifier.notify?(graph(status: :succeeded, trigger: nil), config())
    end

    test "suppresses cron successes unless the workflow is allowlisted" do
      run = graph(run_id: "digest-100-2", status: :succeeded, trigger: %{kind: :cron})

      refute RunNotifier.notify?(run, config())
      assert RunNotifier.notify?(run, config(slack_notify_cron_workflows: ["digest"]))
    end

    test "notifies on cron failures by default and suppresses them when disabled" do
      # A store round-trip leaves the kind string-keyed; it must still be
      # treated as cron.
      run = graph(run_id: "babysit-dispatch-100-2", status: :failed, trigger: %{"kind" => "cron"})

      assert RunNotifier.notify?(run, config())
      refute RunNotifier.notify?(run, config(slack_notify_cron_failures: false))
    end

    test "a tight-interval cron success stays quiet even when failures are enabled" do
      run = graph(run_id: "babysit-dispatch-100-2", status: :succeeded, trigger: %{kind: :cron})

      refute RunNotifier.notify?(run, config(slack_notify_cron_failures: true))
    end

    test "the wildcard allowlist notifies every cron success" do
      run = graph(run_id: "babysit-dispatch-100-2", status: :succeeded, trigger: %{kind: :cron})

      refute RunNotifier.notify?(run, config())
      assert RunNotifier.notify?(run, config(slack_notify_cron_workflows: ["*"]))
    end
  end

  describe "workflow_name/1" do
    test "strips the numeric run-id suffix to recover the workflow slug" do
      assert RunNotifier.workflow_name("babysit-dispatch-1780166452589-58") == "babysit-dispatch"
      assert RunNotifier.workflow_name("triage-100-2") == "triage"
    end
  end

  describe "build_payload/2" do
    test "headers a succeeded run and links run details to the room root when no thread opened" do
      payload =
        RunNotifier.build_payload(
          graph(run_id: "triage-100-2", status: :succeeded, trigger: %{kind: :manual}),
          "https://room.ix.dev"
        )

      [header | _] = payload["blocks"]
      assert header["type"] == "header"
      assert header["text"]["text"] =~ "triage"
      assert header["text"]["text"] =~ "finished"
      assert payload["text"] =~ "Symphony: triage finished"

      # No agent thread on the graph, so the link falls back to the room root.
      run_button = button_with_text(payload, "Run details")
      assert run_button["url"] == "https://room.ix.dev/"
    end

    test "deep-links run details to the run's room backend and latest thread" do
      payload =
        RunNotifier.build_payload(
          graph(
            run_id: "triage-100-2",
            status: :succeeded,
            trigger: %{kind: :manual},
            nodes: %{"n0" => agent_node("n0", "thread_abc")}
          ),
          "https://room.ix.dev/"
        )

      run_button = button_with_text(payload, "Run details")

      # server_id is the registered backend id (Provision.backend_id), encoded
      # like the room client's encodeURIComponent links; the trailing slash on
      # the base is trimmed.
      assert run_button["url"] ==
               "https://room.ix.dev/#/s/symphony%3Atriage-100-2%3Aroom/t/thread_abc"
    end

    test "adds a Linear button from the trigger and marks the run failed" do
      payload =
        RunNotifier.build_payload(
          graph(
            run_id: "triage-100-2",
            status: :failed,
            trigger: %{kind: :linear, identifier: "ENG-9", url: "https://linear.app/indexable/issue/ENG-9"}
          ),
          nil
        )

      [header | _] = payload["blocks"]
      assert header["text"]["text"] =~ "failed"

      linear_button = button_with_text(payload, "ENG-9")
      assert linear_button["url"] == "https://linear.app/indexable/issue/ENG-9"
      # No room url was given, so there is no run-details button.
      assert is_nil(button_with_text(payload, "Run details"))
    end
  end

  describe "content sections" do
    test "posts a sink node's reserved summary output as message content" do
      payload =
        RunNotifier.build_payload(
          graph(
            status: :succeeded,
            trigger: %{kind: :cron},
            nodes: %{
              "gather" => exec_node("gather", %{"summary" => "interior digest"}),
              "digest" => exec_node("digest", %{"summary" => "*hello* from the digest"}, ["gather"])
            }
          ),
          nil
        )

      texts = section_texts(payload)
      assert "*hello* from the digest" in texts
      # Interior node output is plumbing, not publishable content.
      refute "interior digest" in texts
    end

    test "a sink without a string summary adds no content" do
      base = graph(status: :succeeded, trigger: %{kind: :cron})

      for output <- [nil, "raw tail", %{"summary" => 42}, %{"report" => "x"}, %{"summary" => ""}] do
        payload = RunNotifier.build_payload(%{base | nodes: %{"n" => exec_node("n", output)}}, nil)
        assert length(section_texts(payload)) == 1, "unexpected content for output #{inspect(output)}"
      end
    end

    test "content is truncated to Slack's 3000-char section cap" do
      long = String.duplicate("a", 4_000)

      payload =
        RunNotifier.build_payload(
          graph(status: :succeeded, trigger: %{kind: :cron}, nodes: %{"n" => exec_node("n", %{"summary" => long})}),
          nil
        )

      [_summary, content] = section_texts(payload)
      assert byte_size(content) <= 3_000
      assert String.ends_with?(content, "...")
    end

    test "a failed run posts no content even when a sink carries a summary" do
      payload =
        RunNotifier.build_payload(
          graph(status: :failed, trigger: %{kind: :cron}, nodes: %{"n" => exec_node("n", %{"summary" => "partial"})}),
          nil
        )

      refute "partial" in section_texts(payload)
    end
  end

  defp section_texts(payload) do
    for %{"type" => "section", "text" => %{"text" => text}} <- payload["blocks"], do: text
  end

  defp button_with_text(payload, text) do
    payload["blocks"]
    |> Enum.find(%{}, &(&1["type"] == "actions"))
    |> Map.get("elements", [])
    |> Enum.find(fn el -> el["text"]["text"] == text end)
  end
end
