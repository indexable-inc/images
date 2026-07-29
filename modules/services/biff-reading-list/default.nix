# Biff 2 reading list: a single-binary Clojure web application on SQLite.
#
# The application closure is fully locked at build time (Maven artifacts from
# `deps-lock.json`, the sqldef migration tool from the store), so the unit
# fetches nothing when it starts. Everything mutable -- the SQLite database,
# the rendered schema, and the generated cookie secret -- lives in the state
# directory, which is the only writable path `ProtectSystem = "strict"` leaves.
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkEnableOption
    mkIf
    mkOption
    mkPackageOption
    types
    ;
  cfg = config.services.biff-reading-list;

  stateDirectory = "biff-reading-list";
  statePath = "/var/lib/${stateDirectory}";

  # Biff reads the session cookie key from a file, so the secret has to exist
  # before the application starts and has to survive restarts (a regenerated
  # key silently signs every user out). Generating it in the unit rather than
  # baking it keeps it out of the Nix store. Written to a 0400 temporary and
  # renamed, so a crash mid-write can never leave a truncated key behind.
  ensureCookieSecret = ix.writeRustApplication pkgs {
    name = "biff-reading-list-cookie-secret";
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
              eprintln!("biff-reading-list-cookie-secret: {error}");
              std::process::exit(1);
          }
      }
    '';
  };
in {
  options.services.biff-reading-list = {
    enable = mkEnableOption "the Biff reading-list application";

    package = mkPackageOption pkgs "biff-reading-list" {};

    port = mkOption {
      type = types.port;
      default = 8080;
      description = "Port the reading-list HTTP listener answers on.";
    };

    host = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = ''
        Bind address passed to the application as `HOST`. Exposure is bounded
        by the guest firewall, which only opens {option}`port`.
      '';
    };
  };

  config = mkIf cfg.enable {
    users.groups.biff = {};
    users.users.biff = {
      isSystemUser = true;
      group = "biff";
    };

    ix.networking.expose.http = {
      inherit (cfg) port;
      description = "Biff reading-list HTTP";
    };

    ix.healthChecks = {
      biff-reading-list.unit = "biff-reading-list";
      biff-reading-list-http = {
        description = "Biff and SQLite answer a reading-list request";
        http = {
          inherit (cfg) port;
          path = "/";
        };
      };
    };

    systemd.services.biff-reading-list = {
      description = "Biff reading-list example";
      wantedBy = ["multi-user.target"];
      after = ["network.target"];
      environment = {
        HOST = cfg.host;
        PORT = toString cfg.port;
        COOKIE_SECRET_FILE = "${statePath}/cookie-secret";
        SQLITE_DB_PATH = "${statePath}/reading-list.db";
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
          ExecStartPre = lib.getExe ensureCookieSecret;
          ExecStart = lib.getExe' cfg.package "biff-reading-list";
          StateDirectory = stateDirectory;
          StateDirectoryMode = "0750";
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
