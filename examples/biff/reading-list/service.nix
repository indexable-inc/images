{biffApp}: {
  ix,
  lib,
  pkgs,
  ...
}: let
  httpPort = 8080;
  stateDirectory = "biff-reading-list";
  statePath = "/var/lib/${stateDirectory}";
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
  users.groups.biff = {};
  users.users.biff = {
    isSystemUser = true;
    group = "biff";
  };

  ix.networking.expose.http = {
    port = httpPort;
    description = "Biff reading-list HTTP";
  };

  ix.healthChecks = {
    biff-reading-list.unit = "biff-reading-list";
    biff-reading-list-http = {
      description = "Biff and SQLite answer a reading-list request";
      http = {
        port = httpPort;
        path = "/";
      };
    };
  };

  systemd.services.biff-reading-list = {
    description = "Biff reading-list example";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    environment = {
      HOST = "0.0.0.0";
      PORT = toString httpPort;
      COOKIE_SECRET_FILE = "${statePath}/cookie-secret";
      SQLITE_DB_PATH = "${statePath}/reading-list.db";
      SQLITE_SCHEMA_PATH = "${statePath}/schema.sql";
    };
    path = [pkgs.sqldef];
    serviceConfig =
      ix.systemdHardening
      // {
        User = "biff";
        Group = "biff";
        ExecStartPre = lib.getExe ensureCookieSecret;
        ExecStart = "${biffApp}/bin/biff-reading-list";
        StateDirectory = stateDirectory;
        StateDirectoryMode = "0750";
        WorkingDirectory = statePath;
        Restart = "on-failure";
        RestartSec = "2s";
        # clj-nix's launcher shell reports the JVM's normal SIGTERM as 143.
        SuccessExitStatus = [143];
      };
  };
}
