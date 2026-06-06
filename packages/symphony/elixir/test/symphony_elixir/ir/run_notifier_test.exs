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

  defp button_with_text(payload, text) do
    payload["blocks"]
    |> Enum.find(%{}, &(&1["type"] == "actions"))
    |> Map.get("elements", [])
    |> Enum.find(fn el -> el["text"]["text"] == text end)
  end
end
