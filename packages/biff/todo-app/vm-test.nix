# Boots the real Todo App under the shipped `services.biff-todo-app` module and
# drives it end to end (HTTP, sign-in, SQLite, Datastar in a real browser,
# restart, and SIGKILL recovery).
#
# The node runs the module in isolation rather than a full ix image: only the
# two `ix` helpers the module actually consumes are supplied as specialArgs,
# and `ix.networking.expose` / `ix.healthChecks` are stubbed as free-form
# options, so the test exercises the module's own config without pulling in the
# platform module tree.
{
  ix,
  paths,
  pkgs,
}: let
  packages = ix.packageSetFor pkgs;
  browserPython = pkgs.python3.withPackages (python: [python.selenium]);
in
  pkgs.testers.runNixOSTest {
    name = "biff-todo-app";

    node.specialArgs.ix = {
      inherit (ix) systemdHardening writeRustApplication;
    };
    nodes.server = {lib, ...}: {
      imports = [(paths.modules + "/services/biff-todo-app")];
      options.ix = {
        networking.expose = lib.mkOption {
          type = lib.types.attrsOf lib.types.anything;
          default = {};
        };
        healthChecks = lib.mkOption {
          type = lib.types.attrsOf lib.types.anything;
          default = {};
        };
      };
      config = {
        # `package` is set from the caller's package set: the test node runs a
        # stock nixpkgs instance, which carries no repo overlay for
        # `mkPackageOption` to resolve the default against.
        services.biff-todo-app = {
          enable = true;
          package = packages.biff-todo-app;
        };
        environment.etc."todo-app/browser-test.py".source = ./browser-test.py;
        environment.systemPackages = [
          browserPython
          pkgs.firefox
          pkgs.geckodriver
          pkgs.sqlite
        ];
        virtualisation = {
          memorySize = 3072;
          diskSize = 4096;
        };
      };
    };

    testScript = ''
      import re
      import shlex

      database = "/var/lib/biff-todo-app/todo-app.db"
      schema = "/var/lib/biff-todo-app/schema.sql"
      secret = "/var/lib/biff-todo-app/cookie-secret"
      base_url = "http://localhost:8080"

      def shell_command(args):
          return " ".join(shlex.quote(str(arg)) for arg in args)

      def csrf_token(path):
          token = server.succeed(
              "sed -n 's/.*name=\"__anti-forgery-token\" value=\"\\([^\"]*\\)\".*/\\1/p' "
              + shlex.quote(path)
          ).strip()
          assert token
          return token

      server.start()
      server.wait_for_unit("biff-todo-app.service")
      server.wait_for_open_port(8080)

      expected_properties = {
          "User": "biff-todo-app",
          "Group": "biff-todo-app",
          "Restart": "on-failure",
          "RestartUSec": "2s",
          "ProtectSystem": "strict",
          "NoNewPrivileges": "yes",
          "PrivateDevices": "yes",
          "UMask": "0077",
      }
      for property_name, expected in expected_properties.items():
          actual = server.succeed(
              f"systemctl show biff-todo-app.service --property={property_name} --value"
          ).strip()
          assert actual == expected, (property_name, actual, expected)

      main_pid = server.succeed(
          "systemctl show biff-todo-app.service --property=MainPID --value"
      ).strip()
      process_uid = server.succeed(f"awk '/^Uid:/ {{print $2}}' /proc/{main_pid}/status").strip()
      assert process_uid == server.succeed("id -u biff-todo-app").strip()
      assert process_uid != "0"

      server.succeed("test \"$(stat -c '%U:%G:%a' /var/lib/biff-todo-app)\" = biff-todo-app:biff-todo-app:750")
      server.succeed(f"test \"$(stat -c '%U:%G:%a' {secret})\" = biff-todo-app:biff-todo-app:400")
      server.succeed(f"test \"$(base64 --decode {secret} | wc -c)\" -eq 16")
      server.succeed(f"test -s {schema}")
      server.succeed(f"test \"$(sqlite3 {database} 'PRAGMA integrity_check;')\" = ok")
      server.succeed(f"test \"$(sqlite3 {database} 'PRAGMA journal_mode;')\" = wal")
      server.fail("ss -ltn | grep -E ':(7888|22)[[:space:]]'")
      server.fail("test -e /var/lib/biff-todo-app/target/bin/sqlite3def")
      server.fail("journalctl -u biff-todo-app.service | grep -F 'Downloading sqlite3def'")
      server.succeed(
          "systemctl show biff-todo-app.service --property=Environment --value "
          "| grep -F 'COOKIE_SECRET_FILE=/var/lib/biff-todo-app/cookie-secret'"
      )
      server.fail(
          "systemctl show biff-todo-app.service --property=Environment --value "
          "| grep -E '(^| )COOKIE_SECRET='"
      )

      server.succeed(
          f"curl --fail --silent --show-error --output /tmp/home {base_url}/"
      )
      server.succeed("grep -F '<title>Todo App</title>' /tmp/home")
      server.succeed("grep -F 'src=\"/js/datastar.js\"' /tmp/home")
      server.fail("grep -E 'https?://[^\"]+' /tmp/home")
      server.succeed(
          f"curl --fail --silent --show-error --output /tmp/datastar.js {base_url}/js/datastar.js"
      )
      server.succeed("test \"$(wc -c < /tmp/datastar.js)\" -gt 30000")
      server.succeed(
          f"curl --fail --silent --show-error --output /tmp/main.css {base_url}/css/main.css"
      )
      server.succeed("test \"$(wc -c < /tmp/main.css)\" -gt 10000")

      email = "vm-test@example.com"
      server.succeed(
          f"curl --fail --silent --show-error --cookie-jar /tmp/cookies "
          f"--output /tmp/signin {base_url}/signin"
      )
      signin_token = csrf_token("/tmp/signin")
      send_status = server.succeed(shell_command([
          "curl", "--silent", "--show-error", "--output", "/dev/null",
          "--dump-header", "/tmp/send-headers", "--write-out", "%{http_code}",
          "--cookie", "/tmp/cookies", "--cookie-jar", "/tmp/cookies",
          "--request", "POST", f"{base_url}/_biff/auth/send-code",
          "--data-urlencode", f"email={email}",
          "--data-urlencode", f"__anti-forgery-token={signin_token}",
      ])).strip()
      assert send_status == "303", send_status

      verify_location = server.succeed(
          "awk 'tolower($1) == \"location:\" {gsub(/\\r/, \"\", $2); print $2}' /tmp/send-headers"
      ).strip()
      assert verify_location.startswith("/signin?verify=code")
      server.succeed(
          f"curl --fail --silent --show-error --cookie /tmp/cookies "
          f"--cookie-jar /tmp/cookies --output /tmp/verify {base_url}{verify_location}"
      )
      verify_token = csrf_token("/tmp/verify")
      code = server.wait_until_succeeds(
          "journalctl -u biff-todo-app.service -o cat "
          "| sed -n 's/.*Your sign-in code is: \\([0-9][0-9]*\\).*/\\1/p' | tail -n 1",
          timeout=30,
      ).strip()
      assert re.fullmatch(r"[0-9]{6}", code)
      verify_status = server.succeed(shell_command([
          "curl", "--silent", "--show-error", "--output", "/dev/null",
          "--write-out", "%{http_code}", "--cookie", "/tmp/cookies",
          "--cookie-jar", "/tmp/cookies", "--request", "POST",
          f"{base_url}/_biff/auth/verify-code",
          "--data-urlencode", f"email={email}",
          "--data-urlencode", f"code={code}",
          "--data-urlencode", f"__anti-forgery-token={verify_token}",
      ])).strip()
      assert verify_status == "303", verify_status

      server.succeed(
          f"curl --fail --silent --show-error --cookie /tmp/cookies "
          f"--output /tmp/app-before-browser {base_url}/app"
      )
      server.succeed("grep -F 'Signed in as' /tmp/app-before-browser")
      server.succeed("grep -F 'vm-test@example.com' /tmp/app-before-browser")
      server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM user;')\" -eq 1")
      server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM todo;')\" -eq 5")
      server.succeed(
          f"test \"$(sqlite3 {database} \"SELECT COUNT(*) FROM biff_sqlite_kv WHERE namespace = 'biff.auth/signin';\")\" -eq 0"
      )

      browser_result = server.succeed("python /etc/todo-app/browser-test.py")
      assert "real Datastar two-tab update passed" in browser_result
      server.succeed("test -s /tmp/biff-todo-app.png")
      server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM todo;')\" -eq 6")
      server.succeed(
          f"test \"$(sqlite3 {database} \"SELECT completed FROM todo WHERE title = 'Created through real Datastar';\")\" -eq 1"
      )

      app_token = csrf_token("/tmp/app-before-browser")
      archive_status = server.succeed(shell_command([
          "curl", "--silent", "--show-error", "--output", "/dev/null",
          "--write-out", "%{http_code}", "--cookie", "/tmp/cookies",
          "--request", "POST", f"{base_url}/app/archive",
          "--header", f"x-csrf-token: {app_token}",
      ])).strip()
      assert archive_status == "204", archive_status
      server.wait_until_succeeds(
          f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM todo WHERE archived = 0;')\" -eq 0",
          timeout=30,
      )

      secret_hash = server.succeed(f"sha256sum {secret} | cut -d ' ' -f1").strip()
      old_pid = server.succeed(
          "systemctl show biff-todo-app.service --property=MainPID --value"
      ).strip()
      server.succeed("systemctl restart biff-todo-app.service")
      server.wait_for_unit("biff-todo-app.service")
      server.wait_for_open_port(8080)
      new_pid = server.succeed(
          "systemctl show biff-todo-app.service --property=MainPID --value"
      ).strip()
      assert new_pid != old_pid
      assert server.succeed(f"sha256sum {secret} | cut -d ' ' -f1").strip() == secret_hash
      server.succeed(
          f"curl --fail --silent --show-error --cookie /tmp/cookies "
          f"--output /tmp/after-restart {base_url}/app"
      )
      server.fail("grep -F 'Created through real Datastar' /tmp/after-restart")
      server.succeed(
          f"test \"$(sqlite3 {database} \"SELECT archived FROM todo WHERE title = 'Created through real Datastar';\")\" -eq 1"
      )

      crashed_invocation = server.succeed(
          "systemctl show biff-todo-app.service --property=InvocationID --value"
      ).strip()
      crashed_pid = server.succeed(
          "systemctl show biff-todo-app.service --property=MainPID --value"
      ).strip()
      server.succeed(f"kill -KILL {crashed_pid}")
      server.wait_until_succeeds(
          "systemctl is-active --quiet biff-todo-app.service && "
          f"test \"$(systemctl show biff-todo-app.service --property=InvocationID --value)\" != {shlex.quote(crashed_invocation)}",
          timeout=60,
      )
      server.wait_for_open_port(8080)
      assert int(server.succeed(
          "systemctl show biff-todo-app.service --property=NRestarts --value"
      ).strip()) >= 1
      server.succeed(
          f"curl --fail --silent --show-error --cookie /tmp/cookies "
          f"--output /tmp/biff-todo-app.html {base_url}/app"
      )
      server.fail("grep -F 'Created through real Datastar' /tmp/biff-todo-app.html")
      server.succeed(
          f"test \"$(sqlite3 {database} \"SELECT archived FROM todo WHERE title = 'Created through real Datastar';\")\" -eq 1"
      )
      server.succeed(f"test \"$(sqlite3 {database} 'PRAGMA integrity_check;')\" = ok")
      assert server.succeed(f"sha256sum {secret} | cut -d ' ' -f1").strip() == secret_hash
      server.fail("journalctl -u biff-todo-app.service | grep -F 'Downloading sqlite3def'")

      server.succeed(f"cp {database} /tmp/biff-todo-app.db")
      server.copy_from_machine("/tmp/biff-todo-app.html")
      server.copy_from_machine("/tmp/biff-todo-app.db")
      server.copy_from_machine("/tmp/biff-todo-app.png")
    '';
  }
