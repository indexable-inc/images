{
  lib,
  stdenv,
  fetchurl,
  makeBinaryWrapper,
  autoPatchelfHook,
  procps,
  ripgrep,
  bubblewrap,
  socat,
  nix,
  gnupg,
  binName ? "claude",
  # Only the flake package set injects the Nushell writer; the overlay eval
  # context does not. The updater is a maintainer-facing flake output, so the
  # overlay build of `pkgs.claude-code` simply omits `passthru.updateScript`.
  writeNushellApplication ? null,
}:

let
  # Version and per-platform SRI hashes are generated, never hand-edited. Bump
  # with `nix run .#claude-code.updateScript -- <version>`, which refetches
  # Anthropic's per-version manifest and rewrites manifest.json. We pin by raw
  # version (not the npm `latest` tag) because Anthropic ships new builds to the
  # `next` prerelease tag days before promoting them to `latest`, and every
  # channel that normally surfaces an upgrade (the built-in updater, `claude
  # doctor`, sadjow/claude-code-nix) only watches `latest`.
  manifest = lib.importJSON ./manifest.json;
  inherit (manifest) version;

  # Env defaults applied through the wrapper, declared as data (single source)
  # and derived into flags below rather than hand-written into the install phase.
  # `--set-default` (not `--set`) so an explicit env or settings.json `env` value
  # still overrides per machine. Two groups:
  #  - Output-truncation caps raised to the CLI's built-in maxima: we run a
  #    trusted config (our own CLAUDE.md / AGENTS.md / hooks / MCP servers), so
  #    prefer full output over pruning. BASH_MAX_OUTPUT_LENGTH default 30000
  #    chars (binary clamp 150000); TASK_MAX_OUTPUT_LENGTH default 32000 (clamp
  #    160000); MAX_MCP_OUTPUT_TOKENS default ~25000 tokens (no clamp).
  #  - Feature toggles on by default fleet-wide: agent teams, still gated behind
  #    the EXPERIMENTAL_ env var in this build.
  wrapperEnvDefaults = {
    BASH_MAX_OUTPUT_LENGTH = 150000;
    TASK_MAX_OUTPUT_LENGTH = 160000;
    MAX_MCP_OUTPUT_TOKENS = 200000;
    CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS = 1;
  };
  envDefaultFlags = lib.concatLists (
    lib.mapAttrsToList (name: value: [
      "--set-default"
      name
      (toString value)
    ]) wrapperEnvDefaults
  );

  inherit (stdenv.hostPlatform) system;
  target =
    manifest.platforms.${system}
      or (throw "claude-code: no prebuilt binary for ${system}; supported: ${lib.concatStringsSep ", " (builtins.attrNames manifest.platforms)}");

  # Primary host is the Anthropic-branded CDN so the source is verifiable; the
  # GCS bucket is the direct origin and stays as a mirror if the CDN is down.
  # The hash pin guarantees both resolve to identical bytes, so this is a
  # mirror list, not a behavioral fallback.
  nativeBinary = fetchurl {
    urls = [
      "https://downloads.claude.ai/claude-code-releases/${version}/${target.slug}/claude"
      "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/${version}/${target.slug}/claude"
    ];
    inherit (target) hash;
  };

  # Refreshes manifest.json from Anthropic's published per-version manifest,
  # converting its hex checksums to the SRI hashes the fetcher pins. The slug
  # map lives here as the single owner; default.nix only reads it back. The
  # updater fails closed unless the manifest's detached GPG signature verifies
  # against the pinned release signing key (release-signing-key.asc, fingerprint
  # 31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE, published at
  # downloads.claude.ai/keys/claude-code.asc), so a spoofed manifest cannot
  # inject hashes for attacker-controlled binaries.
  updateScript =
    if writeNushellApplication == null then
      null
    else
      writeNushellApplication {
        name = "claude-code-update";
        runtimeInputs = [
          nix
          gnupg
        ];
        meta.description = "Refresh packages/claude-code/manifest.json to a signed Claude Code release";
        text = ''
          const base = "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases"
          const signing_key = "${./release-signing-key.asc}"
          const slugs = {
            "aarch64-darwin": "darwin-arm64",
            "x86_64-darwin": "darwin-x64",
            "x86_64-linux": "linux-x64",
            "aarch64-linux": "linux-arm64"
          }

          # Run from the repo root: `nix run .#claude-code.updateScript -- [version]`.
          # Without a version argument it tracks Anthropic's `latest` pointer.
          def main [version?: string] {
            let v = ($version | default (http get $"($base)/latest" | str trim))

            # Download the exact bytes we verify, then parse the same file.
            let work = (mktemp --directory)
            let manifest_path = $"($work)/manifest.json"
            let sig_path = $"($work)/manifest.json.sig"
            http get --raw $"($base)/($v)/manifest.json" | save --force $manifest_path
            http get --raw $"($base)/($v)/manifest.json.sig" | save --force $sig_path

            # Fail closed: only the pinned key lives in this GNUPGHOME, so a
            # zero exit from --verify proves Anthropic signed these exact bytes.
            let gnupghome = (mktemp --directory)
            with-env { GNUPGHOME: $gnupghome } {
              ^gpg --batch --quiet --import $signing_key
              let check = (do { ^gpg --batch --verify $sig_path $manifest_path } | complete)
              if $check.exit_code != 0 {
                error make { msg: $"claude-code: manifest signature verification failed for ($v)\n($check.stderr)" }
              }
            }

            let upstream = (open $manifest_path)
            let platforms = (
              $slugs
              | transpose system slug
              | reduce --fold {} {|row acc|
                  let hex = ($upstream.platforms | get $row.slug | get checksum)
                  let sri = (^nix hash convert --hash-algo sha256 --to sri $hex | str trim)
                  $acc | insert $row.system { slug: $row.slug, hash: $sri }
                }
            )
            let out = "packages/claude-code/manifest.json"
            { version: $v, platforms: $platforms } | to json --indent 2 | save --force $out
            print $"updated ($out) to ($v); signature verified"
          }
        '';
      };
in
stdenv.mkDerivation {
  pname = "claude-code";
  inherit version;

  dontUnpack = true;
  # Stripping rewrites the binary and corrupts the trailer Bun appends to its
  # single-file executables, so the stripped CLI aborts on launch.
  dontStrip = true;
  strictDeps = true;

  nativeBuildInputs = [
    makeBinaryWrapper
  ]
  ++ lib.optional stdenv.hostPlatform.isElf autoPatchelfHook;

  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin $out/libexec

    # 1Password's "CLI access requested" prompt labels the request with the
    # basename of the process that spawns `op`, which is this real binary rather
    # than the wrapper. Keep it in libexec (off PATH, no leading-dot wrapper
    # convention) and name it for the product so the prompt reads "Claude Code"
    # instead of ".claude-unwrapped". The basename is the human-facing product
    # label, independent of the command alias, since it is only what macOS shows.
    # 1Password docs confirm the prompt shows "the process being authorized (for
    # example, iTerm2 or Terminal)", not the code signature or CFBundleName:
    # https://developer.1password.com/docs/cli/app-integration-security/
    helper="$out/libexec/Claude Code"
    install -m755 ${nativeBinary} "$helper"

    # The store output is read-only, so the bundled self-updater can never
    # write; disable it and the install checks, and pin the bundled ripgrep to
    # the Nix one so PATH stays reproducible. The wrapper owns the version pin.
    # Apply our env defaults (see `wrapperEnvDefaults` above).
    #
    # Start in debug mode by default (`--debug`): the CLI writes operational
    # telemetry (HTTP/API timings, auto-mode classifier, MCP/LSP lifecycle,
    # startup phases, permission decisions) to ~/.claude/debug/ for
    # troubleshooting and the optimize history analysis. It does not pollute
    # `claude -p` stdout (verified), and those logs are pruned on the normal
    # cleanupPeriodDays sweep, so set a long retention in settings to keep them.
    makeBinaryWrapper "$helper" $out/bin/${binName} \
      --inherit-argv0 \
      --add-flags --debug \
      --set DISABLE_AUTOUPDATER 1 \
      --set DISABLE_INSTALLATION_CHECKS 1 \
      --set USE_BUILTIN_RIPGREP 0 \
      ${lib.escapeShellArgs envDefaultFlags} \
      --prefix PATH : ${
        lib.makeBinPath (
          [
            procps
            ripgrep
          ]
          ++ lib.optionals stdenv.hostPlatform.isLinux [
            bubblewrap
            socat
          ]
        )
      }

    runHook postInstall
  '';

  passthru = lib.optionalAttrs (updateScript != null) {
    inherit updateScript;
  };

  meta = {
    description = "Claude Code, Anthropic's agentic coding tool in the terminal";
    homepage = "https://www.anthropic.com/claude-code";
    # License omitted rather than `licenses.unfree` to match the proprietary
    # `packages/ix` vendor binary: the per-system flake package set evaluates
    # nixpkgs without `allowUnfree`, so tagging it unfree would block
    # `nix run .#claude-code`. Distribution terms are Anthropic's commercial
    # Claude Code license.
    mainProgram = binName;
    platforms = builtins.attrNames manifest.platforms;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
