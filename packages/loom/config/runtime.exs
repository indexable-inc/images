import Config

# Every knob as an env var, so `iex -S mix` inside the control VM is
# fully configured by the launcher script (see README quickstart).
env = fn name, key ->
  case System.get_env(name) do
    nil -> :ok
    value -> config(:loom, [{key, value}])
  end
end

env.("LOOM_IX_BIN", :ix_bin)
env.("LOOM_CLAUDE_BIN", :claude_bin)
env.("LOOM_PARENT_VM", :parent_vm)
env.("LOOM_PREFLIGHT", :preflight)

split = fn name, key ->
  case System.get_env(name) do
    nil -> :ok
    value -> config(:loom, [{key, String.split(value)}])
  end
end

split.("LOOM_IX_PREFIX", :ix_prefix)
split.("LOOM_RESTORE_ARGS", :restore_args)
split.("LOOM_CLAUDE_ARGS", :claude_args)
