{biffApp}: {
  ix,
  lib,
  pkgs,
  ...
}: let
  httpPort = 8080;
  stateDirectory = "biff-todo-app";
  statePath = "/var/lib/${stateDirectory}";
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
  users.groups.biff = {};
  users.users.biff = {
    isSystemUser = true;
    group = "biff";
  };

  ix.networking.expose.http = {
    port = httpPort;
    description = "Biff Todo App HTTP";
  };

  ix.healthChecks = {
    biff-todo-app.unit = "biff-todo-app";
    biff-todo-app-http = {
      description = "Todo App serves its landing page";
      http = {
        port = httpPort;
        path = "/";
      };
    };
  };

  systemd.services.biff-todo-app = {
    description = "Biff Todo App example";
    wantedBy = ["multi-user.target"];
    after = ["network.target"];
    environment = {
      BASE_URL = "http://localhost:${toString httpPort}";
      BIFF_AUTH_SKIP_CAPTCHA = "true";
      BIFF_PROFILE = "prod";
      COOKIE_SECRET_FILE = "${statePath}/cookie-secret";
      HOST = "0.0.0.0";
      PORT = toString httpPort;
      SECURE = "false";
      SQLITE_DB_PATH = "${statePath}/todo-app.db";
      SQLITE_SCHEMA_PATH = "${statePath}/schema.sql";
    };
    path = [pkgs.sqldef];
    serviceConfig =
      ix.systemdHardening
      // {
        User = "biff";
        Group = "biff";
        EnvironmentFile = "-${statePath}/config.env";
        ExecStartPre = lib.getExe ensureCookieSecret;
        ExecStart = "${biffApp}/bin/biff-todo-app";
        StateDirectory = stateDirectory;
        StateDirectoryMode = "0750";
        UMask = "0077";
        WorkingDirectory = statePath;
        Restart = "on-failure";
        RestartSec = "2s";
        SuccessExitStatus = [143];
      };
  };
}
