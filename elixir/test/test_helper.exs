ExUnit.start()

# Boot a Config snapshot with sensible test defaults so any unit test
# touching Config.get() works without needing the full Application
# supervisor running. Individual tests that need different values can
# inspect/override the ETS table directly.
test_root = Path.join(System.tmp_dir!(), "symphony_test_root_#{System.unique_integer([:positive])}")
File.mkdir_p!(test_root)
File.mkdir_p!(Path.join([test_root, "workflows", "example", "workflows"]))
File.mkdir_p!(Path.join([test_root, "workflows", "example", "skills"]))

File.write!(Path.join([test_root, "workflows", "example", "repositories.yaml"]), """
repositories:
  - name: example
    owner_repo: example/example
    default_branch: main
    primary: true
""")

System.put_env("SYMPHONY_ROOT", test_root)
System.put_env("SYMPHONY_WORKFLOW_PACK", "example")
System.put_env("LINEAR_WORKSPACE_SLUG", "example-org")

{:ok, _config} = SymphonyElixir.Config.start_link([])
{:ok, _phx_pubsub} = Phoenix.PubSub.Supervisor.start_link(name: SymphonyElixir.PubSub)
{:ok, _endpoint} = SymphonyElixirWeb.Endpoint.start_link()
