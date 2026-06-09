# Shared builder for the pi-harnesses collection.
#
# Wraps `pi` with a fixed, build-time posture (which flags, which extensions,
# which model table) and produces a single `bin/<name>` launcher. The engine
# harness (packages/pi-harnesses/engine) keeps its own hardened C launcher for
# the secret-bearing Room posture; this builder targets the developer-facing
# orchestration harnesses (prosecutor, beam) where a clean shell wrapper with
# the built-in tools PRESENT is the right tradeoff.
{
  lib,
  stdenv,
  bash,
  coreutils,
  git,
  nodejs,
  pi-coding-agent,
  # The bare `pi` binary the wrapper execs. Pinned to nixpkgs' `pi-coding-agent`
  # (same posture as the engine harness) so the wrapper never resolves `pi`
  # from the caller's PATH, where a host-level wrapper can inject conflicting
  # flags and extensions. Pass a derivation here to override.
  pi ? pi-coding-agent,
}:
{
  name,
  description ? "pi harness",
  # Declarative model table: { alias = { provider; model; ... }; }.
  models,
  defaultModel ? "claude",
  # Entry extensions: each is copied next to the wrapper and loaded with `-e`.
  extensions ? [ ],
  # Helper modules imported by the entry extensions (copied into share/<name>/lib).
  libFiles ? [ ],
  # Files copied next to the extensions but NOT auto-loaded (e.g. the turn-cap
  # extension that beam branches load by absolute path).
  auxFiles ? [ ],
  # Posture knobs. lockdown=false is the whole point of these harnesses.
  lockdown ? false,
  session ? true,
  headless ? false,
  mode ? null,
  systemPrompt ? null,
  # Extra KEY=VALUE pairs exported in the wrapper environment.
  env ? { },
  # Extra PATH entries available to the wrapper and any child `pi`.
  runtimeInputs ? [ ],
  # Optional node `--test` files run at build time, plus the lib files they import.
  checkFiles ? [ ],
  checkLib ? [ ],
}:
let
  piBin = lib.getExe pi;
  path = lib.makeBinPath (
    [
      coreutils
      git
      pi
    ]
    ++ runtimeInputs
  );

  flags =
    lib.optionals lockdown [
      "--no-builtin-tools"
      "--no-extensions"
      "--no-skills"
    ]
    ++ lib.optional (!session) "--no-session"
    ++ lib.optional headless "--print"
    ++ lib.optionals (headless && mode != null) [
      "--mode"
      mode
    ]
    ++ lib.optionals (systemPrompt != null) [
      "--system-prompt"
      systemPrompt
    ];
  flagsStr = lib.escapeShellArgs flags;

  modelCase = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (
      alias: m:
      "  ${alias}) PI_PROVIDER=${lib.escapeShellArg m.provider}; PI_MODEL=${lib.escapeShellArg m.model}; PI_THINKING=${
          lib.escapeShellArg (m.thinking or "")
        } ;;"
    ) models
  );

  extEnv = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (k: v: "export ${k}=${lib.escapeShellArg (toString v)}") env
  );

  extBasenames = lib.concatStringsSep " " (map (e: lib.escapeShellArg (baseNameOf e)) extensions);

  copyInto =
    dest: files:
    lib.concatMapStringsSep "\n" (f: ''install -Dm644 ${f} "${dest}/${baseNameOf f}"'') files;
in
stdenv.mkDerivation {
  pname = name;
  version = "0.1.0";
  dontUnpack = true;
  strictDeps = true;
  nativeBuildInputs = [ bash ] ++ lib.optional (checkFiles != [ ]) nodejs;
  doCheck = checkFiles != [ ];

  buildPhase = ''
    runHook preBuild
    mkdir -p share/${name}/lib
    ${copyInto "share/${name}" extensions}
    ${copyInto "share/${name}" auxFiles}
    ${copyInto "share/${name}/lib" libFiles}

    cp ${./wrapper.sh.in} wrapper.sh
    substituteInPlace wrapper.sh \
      --replace-fail '@PATH@' ${lib.escapeShellArg path} \
      --replace-fail '@NAME@' ${lib.escapeShellArg name} \
      --replace-fail '@DEFAULT_MODEL@' ${lib.escapeShellArg defaultModel} \
      --replace-fail '@PI@' ${lib.escapeShellArg piBin} \
      --replace-fail '@FLAGS@' ${lib.escapeShellArg flagsStr} \
      --replace-fail '@EXT_BASENAMES@' ${lib.escapeShellArg extBasenames} \
      --replace-fail '@MODEL_CASE@' ${lib.escapeShellArg modelCase} \
      --replace-fail '@EXT_ENV@' ${lib.escapeShellArg extEnv}
    runHook postBuild
  '';

  checkPhase = ''
    runHook preCheck
    cdir=$(mktemp -d)
    ${copyInto "$cdir" checkLib}
    ${copyInto "$cdir" checkFiles}
    ( cd "$cdir" && node --test )
    runHook postCheck
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/bin"
    cp -r share "$out/share"
    install -Dm755 wrapper.sh "$out/bin/${name}"
    runHook postInstall
  '';

  meta = {
    inherit description;
    mainProgram = name;
  };
}
