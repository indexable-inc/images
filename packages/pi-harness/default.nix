{
  lib,
  stdenv,
  buildNpmPackage,
  python3,
  # Pi is not yet packaged in this repo. Until the dependency-intake follow-up
  # lands a pinned `pi` derivation, the wrapper calls `pi` from PATH (the dev
  # image / system already provides it). Pass a derivation here to pin it.
  pi ? null,
  # ix-mcp supplies the ONLY tool surface (python_exec + search_* + calendar_*),
  # built from index/packages/mcp. Pass null to fall back to PATH for local dev.
  ix-mcp ? null,
}:
let
  models = import ./models.nix;
  defaultModel = "claude";

  # Render the declarative model table (models.nix) as C data, so models.nix
  # stays the single source of truth for provider/model selection.
  modelTable = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (
      alias: m:
      "    { ${builtins.toJSON alias}, ${builtins.toJSON m.provider}, ${builtins.toJSON m.model} },"
    ) models
  );
  defaultSystemPrompt = "You are a coding agent. All actions - shell, file IO, HTTP - run through the python_exec tool on a shared Python kernel.";

  # Name exactly the files the build needs, so node_modules/_probe never enter
  # the source closure.
  extensionSrc = lib.fileset.toSource {
    root = ./extension;
    fileset = lib.fileset.unions [
      (./extension + "/ix-mcp-bridge.ts")
      (./extension + "/env.js")
      (./extension + "/env.test.mjs")
      (./extension + "/package.json")
      (./extension + "/package-lock.json")
    ];
  };

  # Build the bridge WITH its npm deps so the shipped extension actually loads:
  # Pi resolves `@modelcontextprotocol/sdk` from node_modules next to the .ts,
  # the same layout proven to work end-to-end. npmDepsHash pins the dep closure;
  # refresh it with `nix run nixpkgs#prefetch-npm-deps -- extension/package-lock.json`.
  extension = buildNpmPackage {
    pname = "ix-mcp-bridge";
    version = "0.1.0";
    src = extensionSrc;
    npmDepsHash = "sha256-Nis7wQLp7wASaEu4n/Cp3pthB3z+9FsTJs5pK3oq77M=";
    # No build script: install the source plus production node_modules verbatim.
    dontNpmBuild = true;
    doCheck = true;
    checkPhase = ''
      runHook preCheck
      npm test
      runHook postCheck
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cp ix-mcp-bridge.ts env.js package.json $out/
      cp -r node_modules $out/node_modules
      runHook postInstall
    '';
  };

  mapper = ./room_event_mapper.py;

  runtimeInputs = [ python3 ] ++ lib.optional (pi != null) pi ++ lib.optional (ix-mcp != null) ix-mcp;
  runtimePath = lib.makeBinPath runtimeInputs;
  piCommand = if pi == null then "pi" else lib.getExe pi;
  pythonCommand = lib.getExe python3;
  isLinux = stdenv.hostPlatform.isLinux;
  hardenerLibraryName = "libpi-harness-harden.so";
  hardenerPath = lib.optionalString isLinux "$out/lib/${hardenerLibraryName}";
in
stdenv.mkDerivation {
  pname = "pi-harness";
  version = "0.1.0";
  dontUnpack = true;
  strictDeps = true;
  doCheck = true;

  buildPhase = ''
        runHook preBuild

    ${lib.optionalString isLinux ''
          cat > harden.c <<'EOF'
          #define _GNU_SOURCE
          #ifdef __linux__
          #include <errno.h>
          #include <stdio.h>
          #include <string.h>
          #include <sys/prctl.h>
          #include <unistd.h>

          static void pi_harness_fail_closed(const char *operation) {
            int err = errno;
            dprintf(STDERR_FILENO, "pi-harness: %s failed: %s\n", operation, strerror(err));
            _exit(126);
          }

          __attribute__((constructor)) static void pi_harness_harden(void) {
            if (prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0) {
              pi_harness_fail_closed("PR_SET_DUMPABLE");
            }
            if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) {
              pi_harness_fail_closed("PR_SET_NO_NEW_PRIVS");
            }
          }
          #endif
      EOF
    ''}

        cat > launcher.c <<'EOF'
        #define _GNU_SOURCE
        #include <errno.h>
        #include <stdio.h>
        #include <stdlib.h>
        #include <string.h>
        #ifdef __linux__
        #include <sys/prctl.h>
        #endif
        #include <unistd.h>

        struct model_config {
          const char *alias;
          const char *provider;
          const char *model;
        };

        static const struct model_config MODEL_TABLE[] = {
    ${modelTable}
        };

        #ifdef __linux__
        static void pi_harness_fail_closed(const char *operation) {
          int err = errno;
          dprintf(STDERR_FILENO, "pi-harness: %s failed: %s\n", operation, strerror(err));
          _exit(126);
        }

        static void pi_harness_harden_current_process(void) {
          if (prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0) {
            pi_harness_fail_closed("PR_SET_DUMPABLE");
          }
          if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) {
            pi_harness_fail_closed("PR_SET_NO_NEW_PRIVS");
          }
        }
        #endif

        static const char *env_or_default(const char *name, const char *fallback) {
          const char *value = getenv(name);
          return (value != NULL && value[0] != '\0') ? value : fallback;
        }

        static const struct model_config *find_model(const char *alias) {
          size_t count = sizeof(MODEL_TABLE) / sizeof(MODEL_TABLE[0]);
          for (size_t i = 0; i < count; i++) {
            if (strcmp(MODEL_TABLE[i].alias, alias) == 0) {
              return &MODEL_TABLE[i];
            }
          }
          return NULL;
        }

        static int set_joined_env(const char *name, const char *first, const char *sep) {
          const char *old = getenv(name);
          if (old == NULL || old[0] == '\0') {
            return setenv(name, first, 1);
          }

          size_t len = strlen(first) + strlen(sep) + strlen(old) + 1;
          char *joined = malloc(len);
          if (joined == NULL) {
            return -1;
          }
          snprintf(joined, len, "%s%s%s", first, sep, old);
          int rc = setenv(name, joined, 1);
          free(joined);
          return rc;
        }

        static int prepend_runtime_path(void) {
          const char *runtime_path = ${builtins.toJSON runtimePath};
          if (runtime_path[0] == '\0') {
            return 0;
          }
          return set_joined_env("PATH", runtime_path, ":");
        }

        static int preload_hardener(void) {
          const char *hardener = "@HARDENER_PATH@";
          if (hardener[0] == '\0') {
            return 0;
          }
          return set_joined_env("LD_PRELOAD", hardener, " ");
        }

        static char *make_default_store_path(void) {
          const char *tmpdir = env_or_default("TMPDIR", "/tmp");
          size_t template_len = strlen(tmpdir) + strlen("/pi-harness.XXXXXX") + 1;
          char *template = malloc(template_len);
          if (template == NULL) {
            return NULL;
          }
          snprintf(template, template_len, "%s/pi-harness.XXXXXX", tmpdir);

          char *dir = mkdtemp(template);
          if (dir == NULL) {
            free(template);
            return NULL;
          }

          size_t store_len = strlen(dir) + strlen("/ix-mcp.sqlite") + 1;
          char *store = malloc(store_len);
          if (store == NULL) {
            free(template);
            return NULL;
          }
          snprintf(store, store_len, "%s/ix-mcp.sqlite", dir);
          free(template);
          return store;
        }

        int main(int argc, char **argv) {
        #ifdef __linux__
          pi_harness_harden_current_process();
        #endif

          const char *alias = env_or_default("PI_HARNESS_MODEL", ${builtins.toJSON defaultModel});
          const struct model_config *cfg = find_model(alias);
          if (cfg == NULL) {
            fprintf(stderr, "pi-harness: unknown model alias '%s'\n", alias);
            return 2;
          }

          if (prepend_runtime_path() != 0 || preload_hardener() != 0) {
            fprintf(stderr, "pi-harness: failed to prepare environment: %s\n", strerror(errno));
            return 125;
          }

          const char *mode = env_or_default("PI_HARNESS_MODE", "json");
          const char *system_prompt = env_or_default(
            "PI_HARNESS_SYSTEM_PROMPT",
            ${builtins.toJSON defaultSystemPrompt}
          );
          const char *pi = env_or_default("PI_HARNESS_PI_BIN", ${builtins.toJSON piCommand});
          const char *extension = ${builtins.toJSON "${extension}/ix-mcp-bridge.ts"};
          const char *python = ${builtins.toJSON pythonCommand};
          const char *mapper = ${builtins.toJSON mapper};

          size_t rest = (argc > 1) ? (size_t)(argc - 1) : 0;
          char **pi_args = calloc(17 + rest, sizeof(char *));
          if (pi_args == NULL) {
            fprintf(stderr, "pi-harness: failed to allocate argv\n");
            return 125;
          }

          size_t i = 0;
          pi_args[i++] = (char *)pi;
          pi_args[i++] = "--no-builtin-tools";
          pi_args[i++] = "--no-extensions";
          pi_args[i++] = "--no-skills";
          pi_args[i++] = "--no-session";
          pi_args[i++] = "--mode";
          pi_args[i++] = (char *)mode;
          pi_args[i++] = "--print";
          pi_args[i++] = "--provider";
          pi_args[i++] = (char *)cfg->provider;
          pi_args[i++] = "--model";
          pi_args[i++] = (char *)cfg->model;
          pi_args[i++] = "--system-prompt";
          pi_args[i++] = (char *)system_prompt;
          pi_args[i++] = "--extension";
          pi_args[i++] = (char *)extension;
          for (int j = 1; j < argc; j++) {
            pi_args[i++] = argv[j];
          }
          pi_args[i] = NULL;
          size_t pi_argc = i;

          if (strcmp(mode, "json") == 0) {
            const char *store = getenv("IX_MCP_STORE");
            char *owned_store = NULL;
            if (store == NULL || store[0] == '\0') {
              owned_store = make_default_store_path();
              if (owned_store == NULL || setenv("IX_MCP_STORE", owned_store, 1) != 0) {
                fprintf(stderr, "pi-harness: failed to prepare IX_MCP_STORE: %s\n", strerror(errno));
                return 125;
              }
              store = owned_store;
            }

            char **mapper_args = calloc(6 + pi_argc, sizeof(char *));
            if (mapper_args == NULL) {
              fprintf(stderr, "pi-harness: failed to allocate mapper argv\n");
              return 125;
            }

            size_t k = 0;
            mapper_args[k++] = (char *)python;
            mapper_args[k++] = (char *)mapper;
            mapper_args[k++] = "--store";
            mapper_args[k++] = (char *)store;
            mapper_args[k++] = "--";
            for (size_t j = 0; j < pi_argc; j++) {
              mapper_args[k++] = pi_args[j];
            }
            mapper_args[k] = NULL;

            execv(python, mapper_args);
            fprintf(stderr, "pi-harness: failed to exec %s: %s\n", python, strerror(errno));
            return 127;
          }

          execvp(pi, pi_args);
          fprintf(stderr, "pi-harness: failed to exec %s: %s\n", pi, strerror(errno));
          return 127;
        }
    EOF

        substituteInPlace launcher.c --replace-fail '@HARDENER_PATH@' "${hardenerPath}"

    ${lib.optionalString isLinux ''
      $CC -shared -fPIC harden.c -o ${hardenerLibraryName}
    ''}
        $CC launcher.c -o pi-harness

        runHook postBuild
  '';

  checkPhase = ''
    runHook preCheck
    PYTHONPATH=${./.} ${pythonCommand} ${./room_event_mapper_test.py}
    runHook postCheck
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/bin" "$out/lib"
    cp pi-harness "$out/bin/pi-harness"
    ${lib.optionalString isLinux ''
      cp ${hardenerLibraryName} "$out/lib/${hardenerLibraryName}"
    ''}
    runHook postInstall
  '';

  meta = {
    description = "Pi engine harness: Pi with built-in tools absent, exposing only the ix-mcp surface, emitting a JSON event stream for Room";
    mainProgram = "pi-harness";
  };
}
