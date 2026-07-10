defmodule SymphonyElixir.GithubAppTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.GithubApp

  describe "configured?/1" do
    test "false when app id is missing" do
      refute GithubApp.configured?(%{
               github_app_id: nil,
               github_app_private_key_pem: "irrelevant"
             })
    end

    test "false when key is missing" do
      refute GithubApp.configured?(%{
               github_app_id: "123",
               github_app_private_key_pem: nil
             })
    end

    test "false when either is empty string" do
      refute GithubApp.configured?(%{
               github_app_id: "",
               github_app_private_key_pem: "pem"
             })

      refute GithubApp.configured?(%{
               github_app_id: "123",
               github_app_private_key_pem: ""
             })
    end

    test "true when both id and key are present" do
      assert GithubApp.configured?(%{
               github_app_id: "123",
               github_app_private_key_pem: "-----BEGIN RSA PRIVATE KEY-----\n..."
             })
    end

    test "false when passed a non-config-shaped term" do
      refute GithubApp.configured?(nil)
      refute GithubApp.configured?(%{})
    end
  end

  describe "best_effort_installation_token/0" do
    # The test env never sets the App credentials, so this exercises the
    # unconfigured path shared by Runtime and ExecRunner: no token, no crash.
    test "returns :none when the app is unconfigured" do
      assert GithubApp.best_effort_installation_token() == :none
    end
  end
end
