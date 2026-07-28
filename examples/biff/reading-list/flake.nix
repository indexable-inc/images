{
  description = "ix example: Biff 2 SQLite reading list";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    clj-nix = {
      url = "github:jlesquembre/clj-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    index,
    nixpkgs,
    clj-nix,
    ...
  }: let
    system = "x86_64-linux";
    pkgs = nixpkgs.legacyPackages."${system}";
    projectCoordinate = "com.example/biff-reading-list";
    biffApp = clj-nix.lib.mkCljApp {
      inherit pkgs;
      modules = [
        {
          projectSrc = ./.;
          name = projectCoordinate;
          version = "0.1.0";
          main-ns = "com.example.reading-list";
        }
      ];
    };
    importIx = index.lib.importIxWasm;
    vm = importIx ./default.ix {inherit index biffApp;};
    vmSmoke = pkgs.testers.runNixOSTest {
      name = "biff-reading-list";

      # The `.ix` evaluation above proves the full Index platform contract.
      # A stock NixOS test VM cannot import that container-oriented platform
      # module, so declare only the option slots written by this service while
      # still using Index's real systemd hardening value.
      node.specialArgs.ix = {
        inherit (index.lib) systemdHardening writeRustApplication;
      };
      nodes.server = {lib, ...}: {
        imports = [
          (import ./service.nix {inherit biffApp;})
        ];
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
          # Runtime DB assertions use the independent sqlite3 CLI; the
          # production service itself still receives only sqldef on PATH.
          environment.systemPackages = [pkgs.sqlite];
          virtualisation = {
            memorySize = 1536;
            diskSize = 2048;
          };
        };
      };

      testScript = ''
        import shlex

        database = "/var/lib/biff-reading-list/reading-list.db"
        schema = "/var/lib/biff-reading-list/schema.sql"

        def shell_command(args):
            return " ".join(shlex.quote(str(arg)) for arg in args)

        server.start()
        server.wait_for_unit("biff-reading-list.service")
        server.wait_for_open_port(8080)

        expected_unit_properties = {
            "User": "biff",
            "Group": "biff",
            "Restart": "on-failure",
            "RestartUSec": "2s",
            "ProtectSystem": "strict",
            "NoNewPrivileges": "yes",
            "PrivateDevices": "yes",
        }
        for property_name, expected in expected_unit_properties.items():
            actual = server.succeed(
                f"systemctl show biff-reading-list.service --property={property_name} --value"
            ).strip()
            assert actual == expected, (property_name, actual, expected)

        main_pid = server.succeed(
            "systemctl show biff-reading-list.service --property=MainPID --value"
        ).strip()
        process_uid = server.succeed(
            f"awk '/^Uid:/ {{print $2}}' /proc/{main_pid}/status"
        ).strip()
        biff_uid = server.succeed("id -u biff").strip()
        assert process_uid == biff_uid
        assert process_uid != "0"

        server.succeed(
            "test \"$(stat -c '%U:%G:%a' /var/lib/biff-reading-list)\" = biff:biff:750"
        )
        server.succeed(
            "systemctl show biff-reading-list.service --property=Environment --value "
            "| grep -F -- '-sqldef-3.11.1/bin'"
        )
        server.fail("test -e /var/lib/biff-reading-list/target/bin/sqlite3def")
        server.fail("journalctl -u biff-reading-list.service | grep -F 'Downloading sqlite3def'")
        server.succeed("test -s /var/lib/biff-reading-list/cookie-secret")
        server.succeed("test \"$(stat -c '%U:%G:%a' /var/lib/biff-reading-list/cookie-secret)\" = biff:biff:400")
        server.succeed("test \"$(base64 --decode /var/lib/biff-reading-list/cookie-secret | wc -c)\" -eq 16")
        server.succeed(f"test -s {schema}")
        server.succeed(f"grep -F 'CREATE TABLE link (' {schema}")
        server.succeed(f"grep -F ') STRICT;' {schema}")
        server.succeed(f"grep -F 'UNIQUE(url)' {schema}")
        server.succeed(f"grep -F 'CREATE INDEX idx_link_created_at ON link(created_at);' {schema}")
        server.succeed(f"test \"$(sqlite3 {database} 'PRAGMA integrity_check;')\" = ok")
        server.succeed(f"test \"$(sqlite3 {database} 'PRAGMA journal_mode;')\" = wal")
        server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM link;')\" -eq 0")

        secret_hash = server.succeed(
            "sha256sum /var/lib/biff-reading-list/cookie-secret | cut -d ' ' -f1"
        ).strip()

        server.succeed(
            "curl --fail --silent --show-error --dump-header /tmp/headers "
            "--cookie-jar /tmp/cookies --output /tmp/home http://127.0.0.1:8080/"
        )
        server.succeed("grep -E '^HTTP/[0-9.]+ 200' /tmp/headers")
        server.succeed("grep -iE '^content-type: text/html; ?charset=utf-8' /tmp/headers")
        server.succeed("grep -F '<title>Reading List</title>' /tmp/home")
        server.succeed("grep -F '<form action=\"/links\" method=\"post\">' /tmp/home")
        token = server.succeed(
            "sed -n 's/.*name=\"__anti-forgery-token\" value=\"\\([^\"]*\\)\".*/\\1/p' /tmp/home"
        ).strip()
        assert token

        def post_status(title, url, csrf=token):
            args = [
                "curl",
                "--silent",
                "--show-error",
                "--output", "/dev/null",
                "--write-out", "%{http_code}",
                "--cookie", "/tmp/cookies",
                "--cookie-jar", "/tmp/cookies",
                "--request", "POST",
                "http://127.0.0.1:8080/links",
                "--data-urlencode", f"title={title}",
                "--data-urlencode", f"url={url}",
            ]
            if csrf is not None:
                args.extend(["--data-urlencode", f"__anti-forgery-token={csrf}"])
            return server.succeed(shell_command(args)).strip()

        assert post_status("Missing token", "https://example.com/missing", None) == "403"
        assert post_status("Wrong token", "https://example.com/wrong", "not-the-token") == "403"

        invalid_links = [
            ("", "https://example.com/blank-title"),
            ("Unsafe link", "javascript:alert(1)"),
            ("FTP link", "ftp://example.com/archive"),
            ("Relative link", "/relative"),
            ("Credentials", "https://user:password@example.com/"),
            ("x" * 201, "https://example.com/long-title"),
            ("Long URL", "https://example.com/" + ("x" * 2048)),
        ]
        for title, url in invalid_links:
            status = post_status(title, url)
            assert status == "400", (title, url, status)

        # Neither CSRF failures nor application validation failures may reach
        # the authorized-write effect.
        server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM link;')\" -eq 0")

        assert post_status("Biff home", "https://biffweb.com/") == "303"
        assert post_status("Biff home updated", "https://biffweb.com/") == "303"
        assert post_status("<script>alert(1)</script>", "https://example.com/xss") == "303"

        server.succeed(
            "curl --fail --silent --show-error --cookie /tmp/cookies "
            "--output /tmp/populated http://127.0.0.1:8080/"
        )
        server.succeed("grep -F 'Biff home updated' /tmp/populated")
        server.succeed("grep -F '&lt;script&gt;alert(1)&lt;/script&gt;' /tmp/populated")
        server.fail("grep -F '<script>alert(1)</script>' /tmp/populated")
        for rejected_text in ["Unsafe link", "FTP link", "Relative link", "Credentials"]:
            server.fail(shell_command(["grep", "-F", rejected_text, "/tmp/populated"]))

        # The unique URL constraint must drive the declared upsert: two posts
        # for biffweb.com become one updated row, alongside the escaped-title row.
        server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM link;')\" -eq 2")
        updated_title = server.succeed(
            shell_command([
                "sqlite3",
                "-batch",
                "-noheader",
                database,
                "SELECT title FROM link WHERE url = 'https://biffweb.com/';",
            ])
        ).strip()
        assert updated_title == "Biff home updated", updated_title

        server.systemctl("restart biff-reading-list.service")
        server.wait_for_unit("biff-reading-list.service")
        server.wait_for_open_port(8080)
        server.succeed("journalctl -u biff-reading-list.service | grep -F 'Shutdown completed.'")
        restarted_secret_hash = server.succeed(
            "sha256sum /var/lib/biff-reading-list/cookie-secret | cut -d ' ' -f1"
        ).strip()
        assert restarted_secret_hash == secret_hash

        server.succeed("curl --fail --silent --show-error --cookie /tmp/cookies --output /tmp/after-restart http://127.0.0.1:8080/")
        server.succeed("grep -F 'Biff home updated' /tmp/after-restart")
        server.succeed("grep -F 'https://biffweb.com/' /tmp/after-restart")
        server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM link;')\" -eq 2")

        # Match the recovery standard used by the more involved server
        # examples: kill the actual JVM, require systemd to start a new
        # invocation, and prove both HTTP and durable state still work.
        crashed_invocation = server.succeed(
            "systemctl show biff-reading-list.service --property=InvocationID --value"
        ).strip()
        crashed_pid = server.succeed(
            "systemctl show biff-reading-list.service --property=MainPID --value"
        ).strip()
        server.succeed(f"kill -KILL {crashed_pid}")
        server.wait_until_succeeds(
            "systemctl is-active --quiet biff-reading-list.service && "
            f"test \"$(systemctl show biff-reading-list.service --property=InvocationID --value)\" != {shlex.quote(crashed_invocation)}",
            timeout=60,
        )
        server.wait_for_open_port(8080)
        assert int(server.succeed(
            "systemctl show biff-reading-list.service --property=NRestarts --value"
        ).strip()) >= 1
        server.succeed(
            "curl --fail --silent --show-error --output /tmp/biff-reading-list.html "
            "http://127.0.0.1:8080/"
        )
        server.succeed("grep -F 'Biff home updated' /tmp/biff-reading-list.html")
        server.succeed("grep -F '&lt;script&gt;alert(1)&lt;/script&gt;' /tmp/biff-reading-list.html")
        server.succeed(f"test \"$(sqlite3 {database} 'PRAGMA integrity_check;')\" = ok")
        server.succeed(f"test \"$(sqlite3 {database} 'SELECT COUNT(*) FROM link;')\" -eq 2")
        final_secret_hash = server.succeed(
            "sha256sum /var/lib/biff-reading-list/cookie-secret | cut -d ' ' -f1"
        ).strip()
        assert final_secret_hash == secret_hash
        server.fail("journalctl -u biff-reading-list.service | grep -F 'Downloading sqlite3def'")

        # Small inspectable witnesses, analogous to the replay artifact from
        # the Minestom VM test. Never export the cookie secret.
        server.succeed(f"cp {database} /tmp/biff-reading-list.db")
        server.copy_from_machine("/tmp/biff-reading-list.html")
        server.copy_from_machine("/tmp/biff-reading-list.db")
      '';
    };
  in {
    packages.${system} = {
      default = biffApp;
      deps-lock = clj-nix.packages.${system}.deps-lock;
    };
    checks.${system}.biff-reading-list-vm = vmSmoke;
    ix.default = vm;
    inherit (vm) nixosConfigurations;
  };
}
