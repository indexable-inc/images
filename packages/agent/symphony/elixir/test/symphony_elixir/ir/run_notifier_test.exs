defmodule SymphonyElixir.IR.RunNotifierTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Config
  alias SymphonyElixir.IR.Attempt
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.RunNotifier

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

  # An exec node carrying the runner's real result wrapper: the decoded
  # SYMPHONY_OUTPUT_FILE document (or the raw stream tail) nests under
  # :output, exactly as ExecRunner.apply_structured_output stores it.
  # `deps` is hand-set so a test can shape sink vs interior directly.
  defp exec_node(id, payload, deps \\ []) do
    %Node{
      id: id,
      ast_origin: {:exec, id},
      kind: :exec,
      inputs: [],
      deps: deps,
      state: :succeeded,
      output: %{kind: :exec, exit_code: 0, output: payload, log: "stream tail"}
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
              "gather" => exec_node("gather", %{"slack_summary" => "interior digest"}),
              "digest" => exec_node("digest", %{"slack_summary" => "*hello* from the digest"}, ["gather"])
            }
          ),
          nil
        )

      texts = section_texts(payload)
      assert "*hello* from the digest" in texts
      # Interior node output is plumbing, not publishable content.
      refute "interior digest" in texts

      # Agent-authored content is untrusted; verbatim mrkdwn disables
      # <!channel>/<@user> mention parsing so it cannot broadcast-ping.
      [content_block] = for %{"text" => %{"text" => "*hello*" <> _} = text} <- payload["blocks"], do: text
      assert content_block["verbatim"] == true
    end

    test "a sink without a string summary adds no content" do
      base = graph(status: :succeeded, trigger: %{kind: :cron})

      # "raw tail" is the no-structured-output case: the runner leaves the
      # stream tail (a binary) under :output when the script wrote no JSON.
      for payload <- ["raw tail", %{"slack_summary" => 42}, %{"report" => "x"}, %{"slack_summary" => ""}] do
        message = RunNotifier.build_payload(%{base | nodes: %{"n" => exec_node("n", payload)}}, nil)
        assert length(section_texts(message)) == 1, "unexpected content for payload #{inspect(payload)}"
      end

      # A node that never produced any output at all.
      bare = %Node{id: "n", ast_origin: {:exec, "n"}, kind: :exec, inputs: [], deps: [], state: :succeeded}
      message = RunNotifier.build_payload(%{base | nodes: %{"n" => bare}}, nil)
      assert length(section_texts(message)) == 1
    end

    test "content is truncated to Slack's 3000-character section cap" do
      long = String.duplicate("a", 4_000)

      payload =
        RunNotifier.build_payload(
          graph(status: :succeeded, trigger: %{kind: :cron}, nodes: %{"n" => exec_node("n", %{"slack_summary" => long})}),
          nil
        )

      [_summary, content] = section_texts(payload)
      assert String.length(content) <= 3_000
      assert String.ends_with?(content, "...")
    end

    test "multibyte content under the character cap is not truncated" do
      # 1200 CJK chars = 3600 bytes: over a byte-measured cap, well under
      # Slack's 3000-character one. It must pass through untouched.
      cjk = String.duplicate("語", 1_200)

      payload =
        RunNotifier.build_payload(
          graph(status: :succeeded, trigger: %{kind: :cron}, nodes: %{"n" => exec_node("n", %{"slack_summary" => cjk})}),
          nil
        )

      assert cjk in section_texts(payload)
    end

    test "multibyte content over the character cap truncates by characters" do
      long = String.duplicate("語", 3_500)

      payload =
        RunNotifier.build_payload(
          graph(status: :succeeded, trigger: %{kind: :cron}, nodes: %{"n" => exec_node("n", %{"slack_summary" => long})}),
          nil
        )

      [_summary, content] = section_texts(payload)
      assert String.length(content) <= 3_000
      assert String.ends_with?(content, "...")
    end

    test "content sections are capped inside Slack's 50-block message limit" do
      # 60 independent sinks (no deps between them); the message must stay
      # under 50 blocks total, so content is capped rather than the whole
      # post failing.
      nodes =
        for i <- 1..60, into: %{} do
          id = "sink-#{String.pad_leading(Integer.to_string(i), 2, "0")}"
          {id, exec_node(id, %{"slack_summary" => "digest #{i}"})}
        end

      payload = RunNotifier.build_payload(graph(status: :succeeded, trigger: %{kind: :cron}, nodes: nodes), nil)

      assert length(payload["blocks"]) <= 50
      # 1 run summary + the capped content sections.
      assert length(section_texts(payload)) == 41
    end

    test "a failed run posts no content even when a sink carries a summary" do
      payload =
        RunNotifier.build_payload(
          graph(status: :failed, trigger: %{kind: :cron}, nodes: %{"n" => exec_node("n", %{"slack_summary" => "partial"})}),
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
