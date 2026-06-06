defmodule SymphonyElixir.Runtime.TriggerTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Runtime.Trigger

  describe "matches?/2" do
    test "cron matches on the declared schedule" do
      declared = %{kind: :cron, schedule: "0 9 * * *", timezone: "UTC", input: %{}}
      assert Trigger.matches?(declared, %{kind: :cron, schedule: "0 9 * * *"})
      refute Trigger.matches?(declared, %{kind: :cron, schedule: "@daily"})
    end

    test "linear matches when the declared label is on the event" do
      declared = %{kind: :linear, label: "[sym] triage"}
      assert Trigger.matches?(declared, %{kind: :linear, labels: ["a", "[sym] triage"]})
      refute Trigger.matches?(declared, %{kind: :linear, labels: ["a", "b"]})
      refute Trigger.matches?(declared, %{kind: :linear, labels: []})
    end

    test "github matches on repo and label together" do
      declared = %{kind: :github_pr_label, repo: "acme/app", label: "ship"}
      assert Trigger.matches?(declared, %{kind: :github_pr_label, repo: "acme/app", label: "ship"})
      refute Trigger.matches?(declared, %{kind: :github_pr_label, repo: "acme/other", label: "ship"})
      refute Trigger.matches?(declared, %{kind: :github_pr_label, repo: "acme/app", label: "hold"})
    end

    test "slack matches the declared channel against name or resolved id" do
      huddle = %{kind: :slack_huddle_completed, channel: "#general"}
      assert Trigger.matches?(huddle, %{kind: :slack_huddle_completed, channel: "#general"})
      assert Trigger.matches?(huddle, %{kind: :slack_huddle_completed, channel: "x", channel_id: "#general"})
      refute Trigger.matches?(huddle, %{kind: :slack_huddle_completed, channel: "#random"})

      mention = %{kind: :slack_app_mention, channel: "C123"}
      assert Trigger.matches?(mention, %{kind: :slack_app_mention, channel_id: "C123"})
      refute Trigger.matches?(mention, %{kind: :slack_app_mention, channel_id: "C999"})
    end

    test "manual always matches its kind" do
      assert Trigger.matches?(%{kind: :manual}, %{kind: :manual, input: %{}})
    end

    test "a nil or mismatched declared trigger never matches" do
      refute Trigger.matches?(nil, %{kind: :manual})
      refute Trigger.matches?(%{kind: :cron, schedule: "x"}, %{kind: :cron})
    end
  end
end
