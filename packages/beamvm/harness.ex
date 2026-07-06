# Persistent BEAM VM harness: one long-lived node that loads and runs the OTP
# applications a Nix-rendered manifest declares, and hot-swaps them in place
# when the manifest changes.
#
# Why this exists: restarting the VM to pick up a new store path drops every
# WebSocket, in-flight run, and supervision tree it hosts. The BEAM is built
# for code replacement, so an update should default to a hot reload and only
# fall back to a restart when the runtime itself (ERTS/Elixir) changed. The
# split of responsibilities that makes this safe:
#
#   * The service unit's command line references ONLY the harness package and
#     a stable manifest path ($XDG config symlink home-manager rewrites), so
#     an app update never changes the unit definition and never restarts it.
#   * A harness/toolchain update DOES change the unit's store path, and the
#     portable-services layer restarts it: exactly the case where hot reload
#     is impossible (new ERTS), handled by construction rather than detection.
#   * `beamvm-ctl reload` (poked by home-manager activation after the symlink
#     moves) makes the running VM re-read the manifest and converge.
#
# Reload semantics per app, in dependency-safe order:
#   removed  -> Application.stop/unload, drop its code paths
#   changed  -> swap code paths, then `:code.modified_modules/0` +
#               `:code.atomic_load/1` (soft-purged, all-or-nothing); a module
#               with lingering old-code processes fails the atomic pass and
#               falls back to per-module purge+load, which kills only the
#               stuck processes (their supervisors restart them on new code)
#   added    -> add code paths, apply release runtime config, ensure_all_started
#
# Toolchain libraries bundled inside a release (elixir, stdlib, logger, ...)
# are skipped when the VM already has that application loaded: the harness and
# every tenant pin the same Erlang/Elixir toolchain in Nix, so the loaded copy
# IS the release's copy, and double code paths would only add shadow-loading
# hazards.
defmodule BeamVM.Harness do
  @socket_name "control.sock"

  def main do
    state_dir = System.fetch_env!("BEAMVM_STATE_DIR")
    manifest_path = System.fetch_env!("BEAMVM_MANIFEST")
    File.mkdir_p!(state_dir)
    socket_path = Path.join(state_dir, @socket_name)
    # A crash leaves the previous socket file behind; listen would EADDRINUSE.
    File.rm(socket_path)

    log("starting: manifest=#{manifest_path} socket=#{socket_path}")
    state = apply_manifest(%{}, read_manifest!(manifest_path))

    {:ok, listener} =
      :gen_tcp.listen(0, [
        :binary,
        packet: :line,
        active: false,
        ifaddr: {:local, String.to_charlist(socket_path)}
      ])

    log("ready: #{map_size(state)} app(s)")
    serve(listener, %{state: state, manifest_path: manifest_path})
  end

  # One connection at a time, handled synchronously: reloads must serialize,
  # and the only clients are activation hooks and an operator's ctl calls.
  defp serve(listener, ctx) do
    {:ok, conn} = :gen_tcp.accept(listener)
    ctx = handle_conn(conn, ctx)
    :gen_tcp.close(conn)
    serve(listener, ctx)
  end

  defp handle_conn(conn, ctx) do
    case :gen_tcp.recv(conn, 0, 10_000) do
      {:ok, line} ->
        {reply, ctx} = handle_command(String.trim(line), ctx)
        :gen_tcp.send(conn, [JSON.encode!(reply), "\n"])
        ctx

      {:error, reason} ->
        log("control connection recv failed: #{inspect(reason)}")
        ctx
    end
  end

  defp handle_command("ping", ctx), do: {%{ok: true, pong: true}, ctx}

  defp handle_command("status", ctx) do
    apps =
      Map.new(ctx.state, fn {app, entry} ->
        {app,
         %{
           started: started?(app),
           paths: length(entry.paths)
         }}
      end)

    {%{ok: true, os_pid: System.pid(), apps: apps}, ctx}
  end

  defp handle_command("reload", ctx) do
    manifest = read_manifest!(ctx.manifest_path)
    state = apply_manifest(ctx.state, manifest)

    changed = Map.keys(manifest) -- Map.keys(ctx.state)
    log("reload complete: #{map_size(state)} app(s), #{length(changed)} added")
    {%{ok: true, os_pid: System.pid(), apps: Map.keys(state)}, %{ctx | state: state}}
  rescue
    err ->
      log("reload FAILED: #{Exception.message(err)}")
      {%{ok: false, error: Exception.message(err)}, ctx}
  end

  defp handle_command(other, ctx) do
    {%{ok: false, error: "unknown command #{inspect(other)}"}, ctx}
  end

  # Manifest shape (rendered by the Nix home-module):
  #   {"apps": {"<app>": {"code_path_globs": [...],
  #                       "start": true,
  #                       "sys_config_globs": [...],
  #                       "runtime_config_globs": [...]}}}
  # Globs, not literal dirs: a release's lib layout (`lib/<dep>-<vsn>/ebin`)
  # is only enumerable after the package is built, and expanding at eval time
  # would be import-from-derivation.
  defp read_manifest!(path) do
    %{"apps" => apps} = path |> File.read!() |> JSON.decode!()

    Map.new(apps, fn {app, spec} ->
      {String.to_atom(app),
       %{
         code_path_globs: Map.fetch!(spec, "code_path_globs"),
         start: Map.get(spec, "start", true),
         sys_config: expand_globs(Map.get(spec, "sys_config_globs", [])),
         runtime_config: expand_globs(Map.get(spec, "runtime_config_globs", []))
       }}
    end)
  end

  defp expand_globs(globs), do: Enum.flat_map(globs, &Path.wildcard/1)

  # Expanded against the tenant's PREVIOUS path set: a library this tenant
  # itself brought last time is being replaced, not double-claimed, so it must
  # not be skipped just because it is loaded. Only libraries loaded from
  # somewhere OUTSIDE the tenant's own previous dirs (the harness toolchain,
  # another tenant) are skipped. Enum.filter, not a `for` with an `app = ...`
  # binding: a nil binding would act as a comprehension filter and silently
  # drop every ebin dir whose parent is not shaped `<app>-<vsn>`.
  defp expand_code_paths(globs, previous_paths) do
    globs
    |> expand_globs()
    |> Enum.filter(fn dir -> keep_code_path?(ebin_app_name(dir), dir, previous_paths) end)
  end

  # lib/<app>-<vsn>/ebin -> :"<app>"; nil for layouts that do not encode one.
  defp ebin_app_name(dir) do
    case dir |> Path.dirname() |> Path.basename() |> String.split("-", parts: 2) do
      [name, _vsn] -> String.to_atom(name)
      _ -> nil
    end
  end

  # Drop a release-bundled library when the VM already has that application
  # loaded from OUTSIDE this tenant's own previous dirs: those are the
  # toolchain apps (elixir, stdlib, logger, kernel, ...) the harness itself
  # booted from the same Nix-pinned toolchain, or a library another tenant
  # already claimed. First tenant wins on shared deps; a version conflict
  # between tenants is a packaging decision for the manifest author, surfaced
  # by the log line.
  defp keep_code_path?(nil, _dir, _previous_paths), do: true

  defp keep_code_path?(app, dir, previous_paths) do
    cond do
      not loaded?(app) ->
        true

      loaded_from_previous?(app, previous_paths) ->
        true

      # Orphaned leftover of a removed tenant: still loaded (stopped), but
      # its ebin is no longer on the code path. Unload the stale spec so the
      # claiming tenant starts from the fresh .app.
      not started?(app) and not on_code_path?(app) ->
        log("reclaiming #{app}: stale spec from a removed tenant")
        Application.unload(app)
        true

      true ->
        log("skipping #{dir}: application #{app} already loaded in this VM")
        false
    end
  end

  defp on_code_path?(app) do
    case :code.lib_dir(app) do
      {:error, _} -> false
      lib_dir -> Enum.member?(:code.get_path(), String.to_charlist(Path.join(to_string(lib_dir), "ebin")))
    end
  end

  defp loaded?(app) do
    Enum.any?(Application.loaded_applications(), fn {name, _, _} -> name == app end)
  end

  # Whether the loaded copy of `app` came from one of this tenant's previous
  # ebin dirs (checked before those dirs are removed from the code path, so
  # `:code.lib_dir/1` still resolves to the old location).
  defp loaded_from_previous?(app, previous_paths) do
    case :code.lib_dir(app) do
      {:error, _} ->
        false

      lib_dir ->
        expanded = Path.expand(to_string(lib_dir))
        Enum.any?(previous_paths, fn p -> Path.expand(Path.dirname(p)) == expanded end)
    end
  end

  defp apply_manifest(state, manifest) do
    removed = Map.keys(state) -- Map.keys(manifest)
    Enum.each(removed, fn app -> remove_app(app, state[app]) end)

    Enum.reduce(manifest, %{}, fn {app, spec}, acc ->
      Map.put(acc, app, converge_app(app, Map.get(state, app), spec))
    end)
  end

  defp remove_app(app, entry) do
    log("removing #{app}")
    # The tenant's whole started set (dependencies ensure_all_started
    # brought up), in reverse order; a dependency another tenant later
    # started itself is not in this list and stays up.
    stop_started(entry.started)
    Application.unload(app)
    Enum.each(entry.paths, &:code.del_path(String.to_charlist(&1)))
  end

  defp converge_app(app, previous, spec) do
    previous_paths = if previous, do: previous.paths, else: []
    previous_started = if previous, do: previous.started, else: []
    new_paths = expand_code_paths(spec.code_path_globs, previous_paths)

    started =
      if new_paths == previous_paths do
        # Same expanded dirs: the store paths did not change, nothing to swap.
        converge_started(app, spec, previous_started)
      else
        Enum.each(previous_paths -- new_paths, &:code.del_path(String.to_charlist(&1)))
        Enum.each(new_paths -- previous_paths, &:code.add_pathz(String.to_charlist(&1)))

        if previous do
          hot_swap_modules(app)
          converge_swapped_spec(app, new_paths, previous_started)
        else
          log("loading #{app} (#{length(new_paths)} code paths)")
        end

        apply_release_config(app, spec)
        converge_started(app, spec, previous_started)
      end

    %{paths: new_paths, start: spec.start, started: started}
  end

  # Converge the running state to the declared one, both directions: `start`
  # flipped off stops the tenant's own started set (a disabled app must not
  # keep serving until a VM restart), and `start` on re-runs
  # ensure_all_started even for an already-running app, which starts any
  # runtime dependency the swapped release added while being a no-op
  # otherwise.
  defp converge_started(app, %{start: true}, previous_started) do
    newly = start_app(app)
    Enum.uniq(previous_started ++ newly)
  end

  defp converge_started(app, %{start: false}, previous_started) do
    if started?(app) do
      log("stopping #{app}: manifest no longer starts it")
      stop_started(previous_started)
    end

    []
  end

  # A hot swap replaces MODULES, but the application controller keeps the
  # .app spec it loaded at first start: a new release that adds a runtime
  # dependency to its .app would never see it started. Detect the spec
  # change and do a loud tenant-level restart (stop, unload, fresh start
  # picks up the new spec) -- correctness over zero-downtime for the rare
  # dep-set change; module-only updates never take this path.
  defp converge_swapped_spec(app, new_paths, previous_started) do
    loaded_deps = Application.spec(app, :applications) || []
    declared = declared_deps(app, new_paths)

    if declared != nil and Enum.sort(declared) != Enum.sort(loaded_deps) do
      log(
        "#{app}: .app dependency set changed (#{inspect(declared -- loaded_deps)} added, " <>
          "#{inspect(loaded_deps -- declared)} removed); restarting the tenant to reload its spec"
      )

      stop_started(previous_started)
      Application.unload(app)
    end

    :ok
  end

  defp declared_deps(app, paths) do
    with dir when is_binary(dir) <- Enum.find(paths, &(ebin_app_name(&1) == app)),
         {:ok, [{:application, ^app, props}]} <-
           :file.consult(String.to_charlist(Path.join(dir, "#{app}.app"))) do
      Keyword.get(props, :applications, [])
    else
      _ -> nil
    end
  end

  # Reverse start order, so dependents stop before their dependencies. Only
  # the apps THIS tenant's ensure_all_started actually started: apps another
  # tenant (or the harness) already ran are not in the list and stay up.
  defp stop_started(started) do
    Enum.each(Enum.reverse(started), fn dep ->
      log("stopping #{dep}")
      Application.stop(dep)
    end)
  end

  # Replay the release boot's config pipeline: sys.config (the baked
  # build-time config from config.exs + prod.exs -- `server: true` for a
  # Phoenix endpoint lives here) DEEP-MERGED with runtime.exs, exactly as
  # the release's config provider would. Deep merge, not two sequential
  # put_all_env passes: runtime.exs typically sets a subset of an app key's
  # keyword list (only `http:` under the endpoint, say), and a plain
  # overwrite of that key silently drops the baked siblings -- observed as
  # symphony booting with `server: true` lost and no HTTP listener.
  # Multi-app config (`config :other_app, ...`) applies globally, which is
  # release semantics too.
  defp apply_release_config(app, spec) do
    base = read_sys_config(app, spec.sys_config)
    runtime = read_runtime_config(app, spec.runtime_config)

    case Config.Reader.merge(base, runtime) do
      [] -> :ok
      merged -> Application.put_all_env(merged, persistent: true)
    end
  end

  # sys.config is one Erlang term: a list of {App, [{Key, Val}]} pairs.
  defp read_sys_config(_app, []), do: []

  defp read_sys_config(app, [path | _] = all) do
    if length(all) > 1, do: log("#{app}: multiple sys.configs matched; using #{path}")
    log("#{app}: applying sys.config #{path}")
    {:ok, [config]} = :file.consult(String.to_charlist(path))
    config
  end

  # `:code.modified_modules/0` lists exactly the loaded modules whose beam on
  # the (just swapped) code path differs from what is running; atomic_load is
  # all-or-nothing and refuses while any of them still has old code, which the
  # soft-purge pass clears for every module no process is stuck on.
  defp hot_swap_modules(app) do
    case :code.modified_modules() do
      [] ->
        log("#{app}: no modified modules")

      mods ->
        Enum.each(mods, &:code.soft_purge/1)

        case :code.atomic_load(mods) do
          :ok ->
            log("#{app}: hot-swapped #{length(mods)} module(s): #{inspect(mods)}")

          {:error, reasons} ->
            # Some process is still executing old code (a purge would kill
            # it). Swap module-by-module with brutal purge: only the stuck
            # processes die, and their supervisors restart them on new code.
            log("#{app}: atomic load failed (#{inspect(reasons)}); per-module brutal swap")

            Enum.each(mods, fn mod ->
              :code.purge(mod)
              :code.load_file(mod)
            end)
        end
    end
  end

  defp read_runtime_config(_app, []), do: []

  defp read_runtime_config(app, [path | _] = all) do
    if length(all) > 1, do: log("#{app}: multiple runtime configs matched; using #{path}")
    log("#{app}: applying runtime config #{path}")
    Config.Reader.read!(path, env: :prod)
  end

  # :temporary, not :permanent: a tenant crashing past its own supervision
  # tree must not take the whole shared VM (and every other tenant) with it.
  # The failure is loud in the log and in `beamvm-ctl status`.
  defp start_app(app) do
    case Application.ensure_all_started(app, :temporary) do
      {:ok, []} ->
        []

      {:ok, started} ->
        log("started #{app} (#{inspect(started)})")
        started

      {:error, reason} ->
        raise "failed to start #{app}: #{inspect(reason)}"
    end
  end

  defp started?(app) do
    Enum.any?(Application.started_applications(), fn {name, _, _} -> name == app end)
  end

  defp log(msg) do
    IO.puts("#{DateTime.utc_now() |> DateTime.to_iso8601()} beamvm: #{msg}")
  end
end
