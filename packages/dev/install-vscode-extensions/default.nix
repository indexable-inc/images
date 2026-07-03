{
  ix,
  lib,
  writeNushellApplication,
}: let
  extensions = (lib.importJSON (ix.paths.root + "/.vscode/extensions.json")).recommendations;
  extensionsJson = builtins.toJSON extensions;
in
  writeNushellApplication {
    name = "install-vscode-extensions";
    meta = {
      description = "Install the VS Code/Cursor extensions recommended by this workspace";
      mainProgram = "install-vscode-extensions";
    };
    text = ''
      # nu
      def editor_cmd [] {
        if (which cursor | is-not-empty) {
          "cursor"
        } else if (which code | is-not-empty) {
          "code"
        } else {
          error make {
            msg: "Install Cursor or VS Code first"
            label: {
              text: "expected a `cursor` or `code` command on PATH"
              span: (metadata $env.PATH).span
            }
          }
        }
      }

      def installed_extensions [editor: string] {
        try {
          ^$editor --list-extensions | lines
        } catch {
          []
        }
      }

      def main [] {
        let editor = editor_cmd
        let desired = '${extensionsJson}' | from json
        let installed = installed_extensions $editor
        let missing = $desired | where {|extension| $extension not-in $installed }

        if ($missing | is-empty) {
          print $"All ($desired | length) recommended extensions are already installed for ($editor)."
          return
        }

        let failed = $missing | each {|extension|
          print $"Installing ($extension)..."
          let installed_extension = try {
            ^$editor --install-extension $extension
            true
          } catch {|err|
            print $"Failed to install ($extension): ($err.msg)"
            false
          }
          if $installed_extension { null } else { $extension }
        } | compact

        if ($failed | is-not-empty) {
          error make {
            msg: $"Failed to install ($failed | length) recommended extensions"
            label: {
              text: ($failed | str join ", ")
              span: (metadata $failed).span
            }
          }
        }

        print $"Installed ($missing | length) recommended extensions for ($editor)."
      }
    '';
  }
