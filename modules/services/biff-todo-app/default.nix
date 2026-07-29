# Biff 2 Todo App: the upstream Biff starter application, packaged as a
# single-binary Clojure web service on SQLite.
#
# The application closure is fully locked at build time, so the unit fetches
# neither Maven artifacts, browser assets, nor migration tools when it starts.
# Mutable state -- the SQLite database, the rendered schema, the generated
# cookie secret, and an optional operator-supplied environment file -- lives in
# the state directory, the only writable path `ProtectSystem = "strict"` leaves.
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkDefault
    mkEnableOption
    mkIf
    mkOption
    mkPackageOption
    types
    ;
  cfg = config.services.biff-todo-app;

  stateDirectory = "biff-todo-app";
  statePath = "/var/lib/${stateDirectory}";

  # Biff reads the session cookie key from a file, so the secret has to exist
  # before the application starts and has to survive restarts (a regenerated
  # key silently signs every user out). Generating it in the unit rather than
  # baking it keeps it out of the Nix store. Written to a 0400 temporary and
  # renamed, so a crash mid-write can never leave a truncated key behind.
  ensureCookieSecret = ix.writeRustApplication pkgs {
    name = "biff-todo-app-cookie-secret";
    text = ''
      fn ensure_secret() -> Result<(), Box<dyn std::error::Error>> {
          use std::fs::{self, File, OpenOptions};
          use std::io;
          use std::os::unix::fs::OpenOptionsExt;
          use std::path::Path;
          use std::process::{Command, Stdio};

          let state_directory = std::env::var_os("STATE_DIRECTORY").ok_or_else(|| {
              io::Error::new(io::ErrorKind::NotFound, "STATE_DIRECTORY is not set")
          })?;
          let state_directory = Path::new(&state_directory);
          let secret_path = state_directory.join("cookie-secret");

          match fs::metadata(&secret_path) {
              Ok(metadata) if metadata.len() > 0 => return Ok(()),
              Ok(_) => {}
              Err(error) if error.kind() == io::ErrorKind::NotFound => {}
              Err(error) => return Err(error.into()),
          }

          let temporary_path = state_directory.join(format!(
              "cookie-secret.tmp.{}",
              std::process::id()
          ));
          let _ = fs::remove_file(&temporary_path);
          let result = (|| -> Result<(), Box<dyn std::error::Error>> {
              let temporary = OpenOptions::new()
                  .write(true)
                  .create_new(true)
                  .mode(0o400)
                  .open(&temporary_path)?;
              let status = Command::new("${lib.getExe pkgs.openssl}")
                  .args(["rand", "-base64", "16"])
                  .stdout(Stdio::from(temporary.try_clone()?))
                  .status()?;
              if !status.success() {
                  return Err(io::Error::other(format!(
                      "openssl rand failed with {status}"
                  ))
                  .into());
              }
              temporary.sync_all()?;
              fs::rename(&temporary_path, &secret_path)?;
              File::open(state_directory)?.sync_all()?;
              Ok(())
          })();
          if result.is_err() {
              let _ = fs::remove_file(&temporary_path);
          }
          result
      }

      fn main() {
          if let Err(error) = ensure_secret() {
              eprintln!("biff-todo-app-cookie-secret: {error}");
              std::process::exit(1);
          }
      }
    '';
  };
in {
  options.services.biff-todo-app = {
    enable = mkEnableOption "the Biff Todo App application";

    package = mkPackageOption pkgs "biff-todo-app" {};

    port = mkOption {
      type = types.port;
      default = 8080;
      description = "Port the Todo App HTTP listener answers on.";
    };

    host = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = ''
        Bind address passed to the application as `HOST`. Exposure is bounded
        by the guest firewall, which only opens {option}`port`.
      '';
    };

    baseUrl = mkOption {
      type = types.str;
      example = "https://todo.example.com";
      description = ''
        Absolute URL the application builds sign-in links and redirects
        against. Defaults to `http://localhost:<port>`, which is what the
        single-VM demo is reached on; a deployment behind a real hostname
        must set it, or emailed sign-in links point at the wrong host.
      '';
    };

    secure = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Mark session cookies `Secure`. Off by default because the demo is
        reached over plain HTTP on loopback, where a `Secure` cookie is never
        sent back and sign-in silently fails. Turn it on behind TLS.
      '';
    };

    skipCaptcha = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Skip the hCaptcha check on the sign-in form. On by default because the
        application ships without captcha keys and would otherwise reject every
        sign-in; turn it off once {option}`environmentFile` supplies them.
      '';
    };

    environmentFile = mkOption {
      type = types.str;
      default = "-${statePath}/config.env";
      description = ''
        `EnvironmentFile` for operator-supplied secrets (SMTP credentials,
        captcha keys) that must not enter the Nix store. The leading `-`
        makes it optional, so the demo starts with no file present.
      '';
    };
  };

  config = mkIf cfg.enable {
    services.biff-todo-app.baseUrl = mkDefault "http://localhost:${toString cfg.port}";

    users.groups.biff = {};
    users.users.biff = {
      isSystemUser = true;
      group = "biff";
    };

    ix.networking.expose.http = {
      inherit (cfg) port;
      description = "Biff Todo App HTTP";
    };

    ix.healthChecks = {
      biff-todo-app.unit = "biff-todo-app";
      biff-todo-app-http = {
        description = "Todo App serves its landing page";
        http = {
          inherit (cfg) port;
          path = "/";
        };
      };
    };

    systemd.services.biff-todo-app = {
      description = "Biff Todo App example";
      wantedBy = ["multi-user.target"];
      after = ["network.target"];
      # The cookie secret is deliberately absent: it is read from
      # COOKIE_SECRET_FILE so it never appears in `systemctl show` output or
      # in any process's environment.
      environment = {
        BASE_URL = cfg.baseUrl;
        BIFF_AUTH_SKIP_CAPTCHA = lib.boolToString cfg.skipCaptcha;
        BIFF_PROFILE = "prod";
        COOKIE_SECRET_FILE = "${statePath}/cookie-secret";
        HOST = cfg.host;
        PORT = toString cfg.port;
        SECURE = lib.boolToString cfg.secure;
        SQLITE_DB_PATH = "${statePath}/todo-app.db";
        SQLITE_SCHEMA_PATH = "${statePath}/schema.sql";
      };
      # sqldef applies the schema at startup. On PATH from the store so the
      # application never downloads a migration binary at runtime.
      path = [pkgs.sqldef];
      serviceConfig =
        ix.systemdHardening
        // {
          User = "biff";
          Group = "biff";
          EnvironmentFile = cfg.environmentFile;
          ExecStartPre = lib.getExe ensureCookieSecret;
          ExecStart = lib.getExe' cfg.package "biff-todo-app";
          StateDirectory = stateDirectory;
          StateDirectoryMode = "0750";
          # Restated rather than inherited from ix.systemdHardening: the
          # database, WAL, and schema files this unit creates are only
          # group/world-unreadable because of it.
          UMask = "0077";
          WorkingDirectory = statePath;
          Restart = "on-failure";
          RestartSec = "2s";
          # A JVM killed by SIGTERM exits 128+15, which is how the application
          # reports an ordinary `systemctl stop`. Without this systemd records
          # every clean stop as a failure.
          SuccessExitStatus = [143];
        };
    };
  };
}
