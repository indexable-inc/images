# Structured application configuration. Nix is the canonical source.
{
  "alacritty-alacritty" = {
    format = "toml";
    value = {
      "font" = {
        "size" = 12.0;
        "normal" = {
          "family" = "FiraCode Nerd Font";
          "style" = "Regular";
        };
        "bold" = {
          "family" = "FiraCode Nerd Font";
          "style" = "Bold";
        };
        "italic" = {
          "family" = "FiraCode Nerd Font";
          "style" = "Italic";
        };
        "bold_italic" = {
          "family" = "FiraCode Nerd Font";
          "style" = "Bold Italic";
          "builtin_box_drawing" = true;
        };
      };
      "window" = {
        "opacity" = 0.95;
        "padding" = {
          "x" = 5;
          "y" = 5;
        };
      };
      "colors" = {
        "primary" = {
          "background" = "#1e1e2e";
          "foreground" = "#cdd6f4";
        };
      };
    };
  };
  "amp-settings" = {
    format = "json";
    value = {
      "amp.dangerouslyAllowAll" = true;
      "amp.experimental.autoHandoff" = {
        "context" = 90;
      };
      "amp.terminal.theme" = "light";
    };
  };
  "atuin-config" = {
    format = "toml";
    value = {
      "auto_sync" = true;
      "enter_accept" = true;
      "style" = "compact";
      "update_check" = false;
      "search_mode" = "skim";
      "workspaces" = true;
      "network_timeout" = 30;
      "network_connect_timeout" = 5;
      "local_timeout" = 2;
      "store_failed" = true;
      "secrets_filter" = true;
      "daemon" = {
        "sync_frequency" = 15;
      };
      "sync" = {
        "records" = true;
      };
    };
  };
  "bacon-prefs" = {
    format = "toml";
    value = {
      "default_job" = "nextest";
      "help_line" = false;
      "on_change_strategy" = "wait_then_restart";
      "exports" = {
        "locations" = {
          "auto" = false;
          "exporter" = "locations";
          "path" = ".bacon-locations";
          "line_format" = "{kind} {path}:{line}:{column} {message}";
        };
      };
      "sound" = {
        "enabled" = true;
        "base_volume" = "40%";
      };
      "keybindings" = {
        "y" = "copy-unstyled-output";
      };
    };
  };
  "cargo-config" = {
    format = "toml";
    value = {
      "unstable" = {
        "checksum-freshness" = true;
        "build-analysis" = true;
        "gc" = true;
        "gitoxide" = "fetch";
      };
      "build" = {
        "analysis" = {
          "enabled" = true;
        };
      };
      "cache" = {
        "global-clean" = {
          "max-src-age" = "1 month";
          "max-crate-age" = "3 months";
          "max-git-co-age" = "1 month";
          "max-git-db-age" = "3 months";
        };
      };
      "net" = {};
      "env" = {
        "RUST_BACKTRACE" = "1";
        "SOURCE_DATE_EPOCH" = "0";
      };
    };
  };
  "claude-global-keybindings" = {
    format = "json";
    value = {
      "$schema" = "https://www.schemastore.org/claude-code-keybindings.json";
      "$docs" = "https://code.claude.com/docs/en/keybindings";
      "bindings" = [
        {
          "context" = "Footer";
          "bindings" = {
            "ctrl+h" = "footer:up";
          };
        }
      ];
    };
  };
  "cursor-cli-config" = {
    format = "json";
    value = {
      "version" = 1;
      "editor" = {
        "vimMode" = false;
      };
      "hasChangedDefaultModel" = false;
      "permissions" = {
        "allow" = [
          "Shell(ls)"
        ];
        "deny" = [];
      };
    };
  };
  "cursor-mcp" = {
    format = "json";
    value = {
      "mcpServers" = {
        "exa" = {
          "url" = "https://mcp.exa.ai/mcp";
        };
        "blender" = {
          "command" = "/Users/andrewgazelka/.nix-profile/bin/blender-mcp";
          "env" = {
            "DISABLE_TELEMETRY" = "true";
          };
        };
        "blender-lab" = {
          "command" = "/Users/andrewgazelka/.nix-profile/bin/blender-lab-mcp";
          "env" = {
            "BLENDER_MCP_PORT" = "9877";
          };
        };
      };
    };
  };
  "cursor-settings" = {
    format = "json";
    value = {
      "[dockerfile]" = {
        "editor.detectIndentation" = false;
        "editor.insertSpaces" = true;
        "editor.tabSize" = 4;
      };
      "[jinja]" = {
        "editor.defaultFormatter" = null;
      };
      "[json]" = {
        "editor.codeActionsOnSave" = {
          "source.fixAll.sortJSON" = "always";
        };
        "editor.defaultFormatter" = "vscode.json-language-features";
        "editor.formatOnSave" = true;
      };
      "[jsonc]" = {
        "editor.defaultFormatter" = "vscode.json-language-features";
        "editor.formatOnSave" = true;
      };
      "[latex]" = {
        "editor.defaultFormatter" = null;
        "editor.formatOnSave" = true;
      };
      "[nix]" = {
        "editor.defaultFormatter" = "jnoortheen.nix-ide";
        "editor.formatOnSave" = true;
      };
      "[python]" = {
        "editor.codeActionsOnSave" = {
          "source.fixAll.ruff" = "explicit";
          "source.organizeImports.ruff" = "explicit";
        };
        "editor.defaultFormatter" = null;
        "editor.formatOnSave" = true;
      };
      "[rust]" = {
        "editor.defaultFormatter" = "rust-lang.rust-analyzer";
        "editor.formatOnPaste" = true;
        "editor.formatOnSave" = true;
        "editor.formatOnType" = false;
      };
      "[svelte]" = {
        "editor.formatOnSave" = false;
      };
      "cursor-retrieval.canAttemptGithubLogin" = false;
      "cursor.composer.shouldAllowCustomModes" = true;
      "cursor.composer.shouldChimeAfterChatFinishes" = true;
      "cursor.composer.usageSummaryDisplay" = "always";
      "cursor.cpp.enablePartialAccepts" = true;
      "cursor.diffs.useCharacterLevelDiffs" = true;
      "cursor.general.enableShadowWorkspace" = true;
      "dotnet.codeLens.enableReferencesCodeLens" = false;
      "editor.accessibilitySupport" = "off";
      "editor.codeLens" = false;
      "editor.cursorBlinking" = "solid";
      "editor.cursorSurroundingLines" = 50;
      "editor.experimental.inlineDiagnostics.enabled" = true;
      "editor.experimental.inlineDiagnostics.suppressLineNumbers" = false;
      "editor.find.loop" = false;
      "editor.folding" = false;
      "editor.foldingImportsByDefault" = true;
      "editor.fontFamily" = "'Berkeley Mono', 'FiraCode Nerd Font', 'Fira Code', monospace";
      "editor.fontLigatures" = true;
      "editor.formatOnPaste" = true;
      "editor.formatOnSave" = true;
      "editor.glyphMargin" = false;
      "editor.inlayHints.enabled" = "on";
      "editor.inlayHints.fontFamily" = "Berkeley Mono";
      "editor.inlayHints.fontSize" = 11;
      "editor.inlayHints.maximumLength" = 1000;
      "editor.inlineSuggest.enabled" = false;
      "editor.lineHeight" = 1.1;
      "editor.lineNumbers" = "relative";
      "editor.minimap.enabled" = false;
      "editor.parameterHints.enabled" = false;
      "editor.renderValidationDecorations" = "on";
      "editor.scrollbar.horizontal" = "hidden";
      "editor.scrollbar.vertical" = "hidden";
      "editor.wordBasedSuggestions" = "off";
      "errorLens.enabled" = true;
      "errorLens.enabledDiagnosticLevels" = [
        "error"
        "warning"
        "info"
      ];
      "errorLens.messageBackgroundMode" = "none";
      "errorLens.replaceLinebreaksSymbol" = "; ";
      "evenBetterToml.formatter.alignComments" = true;
      "evenBetterToml.formatter.arrayTrailingComma" = true;
      "evenBetterToml.formatter.columnWidth" = 100;
      "evenBetterToml.formatter.reorderArrays" = false;
      "evenBetterToml.formatter.reorderInlineTables" = true;
      "evenBetterToml.formatter.reorderKeys" = true;
      "evenBetterToml.taplo.path" = "/Users/andrewgazelka/.nix-profile/bin/taplo";
      "explorer.excludeGitIgnore" = true;
      "explorer.compactFolders" = true;
      "explorer.confirmDelete" = false;
      "explorer.autoReveal" = true;
      "explorer.confirmDragAndDrop" = false;
      "files.associations" = {
        "*.j2" = "jinja";
        "*.jinja" = "jinja";
        "*.jinja2" = "jinja";
        "*.sbpl" = "scheme";
        "*.svx" = "svelte";
      };
      "files.autoSave" = "afterDelay";
      "gitlens.codeLens.authors.enabled" = false;
      "glassit.alpha" = 100;
      "go.toolsManagement.autoUpdate" = true;
      "latex-workshop.formatting.latex" = "latexindent";
      "latex-workshop.latex.recipe.default" = "tectonic";
      "latex-workshop.latex.recipes" = [
        {
          "name" = "tectonic";
          "tools" = [
            "tectonic"
          ];
        }
      ];
      "latex-workshop.latex.tools" = [
        {
          "args" = [
            "--synctex"
            "--keep-logs"
            "%DOC%.tex"
          ];
          "command" = "tectonic";
          "name" = "tectonic";
        }
      ];
      "latex-workshop.latexindent.path" = "latexindent";
      "latex-workshop.view.pdf.viewer" = "tab";
      "makefile.configureOnOpen" = true;
      "outline.showKeys" = false;
      "outline.showProperties" = false;
      "outline.showVariables" = false;
      "problems.showCurrentInStatus" = true;
      "python.languageServer" = "None";
      "redhat.telemetry.enabled" = false;
      "rust-analyzer.assist.preferSelf" = true;
      "rust-analyzer.check.allTargets" = true;
      "rust-analyzer.check.extraArgs" = [];
      "rust-analyzer.check.features" = [];
      "rust-analyzer.checkOnSave" = true;
      "rust-analyzer.completion.autoimport.enable" = true;
      "rust-analyzer.imports.granularity.enforce" = true;
      "rust-analyzer.imports.preferNoStd" = true;
      "rust-analyzer.inlayHints.parameterHints.enable" = false;
      "rust-analyzer.lens.implementations.enable" = false;
      "rust-analyzer.references.excludeImports" = true;
      "rust-analyzer.signatureInfo.documentation.enable" = true;
      "search.caseSensitive" = true;
      "search.exclude" = {
        "**/.git" = true;
        "**/bazel-bin" = true;
        "bazel-*" = true;
      };
      "search.smartCase" = false;
      "scm.defaultViewMode" = "tree";
      "scm.defaultViewSortKey" = "path";
      "security.promptForLocalFileProtocolHandling" = false;
      "svelte.enable-ts-plugin" = true;
      "terminal.integrated.automationProfile.osx" = null;
      "terminal.integrated.defaultProfile.linux" = "nu";
      "terminal.integrated.defaultProfile.osx" = "nu";
      "terminal.integrated.defaultProfile.windows" = "nu";
      "terminal.integrated.fontSize" = 11;
      "terminal.integrated.profiles.osx" = {
        "/run/current-system/sw/bin/nu" = {
          "args" = [
            "-l"
          ];
          "path" = "/run/current-system/sw/bin/bash";
        };
        "Nushell" = {
          "args" = [
            "-l"
          ];
          "path" = "/run/current-system/sw/bin/bash";
        };
        "bash" = {
          "args" = [
            "-l"
          ];
          "icon" = "terminal-bash";
          "path" = "bash";
        };
        "fish" = {
          "args" = [
            "-l"
          ];
          "path" = "fish";
        };
        "nu" = {
          "path" = "/run/current-system/sw/bin/nu";
        };
        "pwsh" = {
          "icon" = "terminal-powershell";
          "path" = "pwsh";
        };
        "tmux" = {
          "icon" = "terminal-tmux";
          "path" = "tmux";
        };
        "zsh" = {
          "args" = [
            "-l"
          ];
          "path" = "zsh";
        };
      };
      "vim.camelCaseMotion.enable" = true;
      "vim.easymotion" = true;
      "vim.hlsearch" = true;
      "vim.ignorecase" = false;
      "vim.incsearch" = true;
      "vim.leader" = "<space>";
      "vim.normalModeKeyBindingsNonRecursive" = [
        {
          "after" = [
            "<leader>"
            "<leader>"
            "w"
          ];
          "before" = [
            "f"
          ];
        }
        {
          "after" = [
            "<leader>"
            "<leader>"
            "b"
          ];
          "before" = [
            "F"
          ];
        }
        {
          "after" = [
            "<leader>"
            "<leader>"
            "e"
          ];
          "before" = [
            "<leader>"
            "e"
          ];
        }
        {
          "after" = [
            "<leader>"
            "<leader>"
            "f"
          ];
          "before" = [
            "<leader>"
            "f"
          ];
        }
        {
          "after" = [
            "<leader>"
            "<leader>"
            "j"
          ];
          "before" = [
            "<leader>"
            "j"
          ];
        }
        {
          "after" = [
            "<leader>"
            "<leader>"
            "k"
          ];
          "before" = [
            "<leader>"
            "k"
          ];
        }
        {
          "after" = [
            "<leader>"
            "<leader>"
            "s"
          ];
          "before" = [
            "<leader>"
            "s"
          ];
        }
      ];
      "vim.smartcase" = false;
      "vim.sneakUseIgnorecaseAndSmartcase" = true;
      "vim.useCtrlKeys" = true;
      "vim.useSystemClipboard" = true;
      "vim.wrapscan" = false;
      "window.autoDetectColorScheme" = true;
      "window.title" = "\${rootName}";
      "window.zoomLevel" = -1;
      "workbench.editor.focusRecentEditorAfterClose" = false;
      "workbench.editor.pinnedTabSizing" = "shrink";
      "workbench.editor.tabActionCloseVisibility" = false;
      "workbench.editor.tabActionUnpinVisibility" = false;
      "workbench.editor.tabSizing" = "shrink";
      "workbench.iconTheme" = "vscode-jetbrains-icon-theme-2023-auto";
      "workbench.list.rowHeight" = 18;
      "workbench.tree.indent" = 8;
      "workbench.tree.renderIndentGuides" = "none";
      "rust-analyzer.inlayHints.bindingModeHints.enable" = true;
      "rust-analyzer.inlayHints.genericParameterHints.lifetime.enable" = true;
      "rust-analyzer.inlayHints.implicitDrops.enable" = true;
      "rust-analyzer.inlayHints.implicitSizedBoundHints.enable" = true;
      "cursorpyright.analysis.inlayHints.genericTypes" = true;
      "git.blame.editorDecoration.enabled" = false;
      "git.showActionButton" = {
        "commit" = false;
        "publish" = false;
        "sync" = false;
      };
      "git.showCommitInput" = false;
      "gitlens.blame.format" = "\${author|2?}";
      "gitlens.blame.compact" = true;
      "gitlens.blame.avatars" = true;
      "gitlens.blame.heatmap.enabled" = false;
      "gitlens.blame.highlight.enabled" = false;
      "gitlens.blame.separateLines" = false;
      "gitlens.blame.ignoreWhitespace" = true;
      "gitlens.blame.toggleMode" = "file";
      "gitlens.defaultDateFormat" = "MMM D, YYYY";
      "gitlens.defaultDateShortFormat" = "M/D/YY";
      "javascript.inlayHints.functionLikeReturnTypes.enabled" = true;
      "javascript.inlayHints.propertyDeclarationTypes.enabled" = true;
      "javascript.inlayHints.variableTypes.enabled" = true;
      "typescript.inlayHints.enumMemberValues.enabled" = true;
      "typescript.inlayHints.functionLikeReturnTypes.enabled" = true;
      "typescript.inlayHints.parameterTypes.enabled" = true;
      "typescript.inlayHints.variableTypes.enabled" = true;
      "git.openRepositoryInParentFolders" = "never";
      "javascript.inlayHints.parameterTypes.enabled" = true;
      "typescript.inlayHints.propertyDeclarationTypes.enabled" = true;
      "javascript.inlayHints.parameterNames.enabled" = "all";
      "window.nativeTabs" = false;
      "window.openFoldersInNewWindow" = "on";
      "cursor.inlineDiff.enablePerformanceProtection" = false;
      "claudeCode.initialPermissionMode" = "bypassPermissions";
      "claudeCode.selectedModel" = "claude-opus-4-5-20251101";
      "claudeCode.allowDangerouslySkipPermissions" = true;
      "claudeCode.preferredLocation" = "panel";
      "workbench.activityBar.location" = "hidden";
      "workbench.editorAssociations" = {
        "*.md" = "vscode.markdown.preview.editor";
      };
      "jupyter.askForKernelRestart" = false;
      "window.commandCenter" = true;
      "remote.SSH.remotePlatform" = {
        "main" = "linux";
        "hc1" = "linux";
        "hil-compute-1" = "linux";
      };
      "rust-analyzer.server.path" = "rust-analyzer";
      "direnv.restart.automatic" = true;
      "cursor.terminal.usePreviewBox" = true;
      "remote.autoForwardPortsSource" = "hybrid";
      "update.releaseTrack" = "dev";
      "update.mode" = "silentlyApplyOnQuit";
      "gitlens.currentLine.enabled" = true;
      "gitlens.currentLine.format" = "\${author}, \${agoOrDate} • \${message}";
      "gitlens.currentLine.scrollable" = false;
      "gitlens.hovers.avatars" = true;
      "gitlens.hovers.avatarSize" = 24;
      "window.density.editorTabHeight" = "compact";
      "workbench.preferredLightColorTheme" = "Islands Light";
      "markdown.preview.lineHeight" = 1.2;
      "nix.enableLanguageServer" = true;
      "nix.serverPath" = "nixd";
      "nix.formatterPath" = "nixfmt";
      "nix.serverSettings" = {
        "nixd" = {
          "formatting" = {
            "command" = [
              "nixfmt"
            ];
          };
          "options" = {
            "nix-darwin" = {
              "expr" = "(builtins.getFlake \"/Users/andrewgazelka/.config/nix\").darwinConfigurations.hydra.options";
            };
            "home-manager" = {
              "expr" = "(builtins.getFlake \"/Users/andrewgazelka/.config/nix\").darwinConfigurations.hydra.options.home-manager.users.type.getSubOptions []";
            };
            "nixpkgs" = {
              "expr" = "import (builtins.getFlake \"/Users/andrewgazelka/.config/nix\").inputs.nixpkgs { }";
            };
          };
        };
      };
      "workbench.navigationControl.enabled" = false;
      "breadcrumbs.enabled" = false;
      "cursor.composer.queueMessageDefaultBehavior" = "stop-and-send";
      "workbench.preferredDarkColorTheme" = "Islands Dark";
      "vim.disableExtension" = true;
      "extensions.experimental.affinity" = {
        "asvetliakov.vscode-neovim" = 1;
      };
    };
  };
  "direnv-direnv" = {
    format = "toml";
    value = {
      "global" = {
        "log_format" = "";
      };
    };
  };
  "iamb-config" = {
    format = "toml";
    value = {
      "profiles" = {
        "default" = {
          "user_id" = "@andrewgazelka:matrix.org";
        };
      };
      "settings" = {
        "reaction_display" = true;
        "reaction_shortcode_display" = false;
        "read_receipt_send" = true;
        "read_receipt_display" = true;
        "typing_notice_send" = true;
        "typing_notice_display" = true;
        "message_shortcode_display" = false;
        "message_user_color" = true;
        "default_room" = "";
      };
      "dirs" = {};
      "macros" = {};
    };
  };
  "jj-config" = {
    format = "toml";
    value = {
      "user" = {
        "name" = "Andrew Gazelka";
        "email" = "andrew.gazelka@gmail.com";
      };
      "ui" = {
        "default-command" = "log";
        "diff-formatter" = [
          "difft"
          "$left"
          "$right"
        ];
        "pager" = "delta";
      };
      "colors" = {
        "diff removed" = {
          "fg" = "red";
        };
        "diff added" = {
          "fg" = "green";
        };
      };
      "snapshot" = {
        "max-new-file-size" = "50MiB";
      };
      "merge-tools" = {
        "difftastic" = {
          "program" = "difft";
        };
      };
    };
  };
  "tap-config" = {
    format = "toml";
    value = {
      "keybinds" = {
        "editor" = "Alt-e";
      };
      "timing" = {
        "escape_timeout_ms" = 50;
      };
    };
  };
  "zed-keymap" = {
    format = "json";
    value = [
      {
        "bindings" = {
          "ctrl-n" = "menu::SelectNext";
          "ctrl-p" = "menu::SelectPrevious";
        };
      }
      {
        "context" = "Editor";
        "bindings" = {
          "ctrl-n" = "editor::MoveDown";
          "ctrl-p" = "editor::MoveUp";
        };
      }
      {
        "context" = "Editor && showing_completions";
        "bindings" = {
          "ctrl-n" = "editor::ContextMenuNext";
          "ctrl-p" = "editor::ContextMenuPrevious";
        };
      }
      {
        "context" = "Editor";
        "bindings" = {
          "ctrl-j" = "editor::Hover";
          "ctrl-shift-j" = "editor::GoToDefinition";
        };
      }
      {
        "context" = "Editor && vim_mode == normal";
      }
      {
        "context" = "Editor";
        "bindings" = {
          "cmd--" = null;
        };
      }
      {
        "context" = "Editor";
        "bindings" = {
          "cmd-p" = "editor::ShowSignatureHelp";
        };
      }
      {
        "bindings" = {
          "cmd-e" = "file_finder::Toggle";
          "cmd-1" = "workspace::ToggleLeftDock";
        };
      }
      {
        "bindings" = {
          "cmd-i" = "agent::Toggle";
        };
      }
      {
        "context" = "ProjectPanel";
        "bindings" = {
          "cmd-backspace" = [
            "project_panel::Delete"
            {
              "skip_prompt" = true;
            }
          ];
        };
      }
      {
        "context" = "BufferSearchBar";
        "bindings" = {
          "shift-enter" = "search::SelectPreviousMatch";
        };
      }
      {
        "context" = "BufferSearchBar && !in_replace > Editor";
        "bindings" = {
          "shift-enter" = "search::SelectPreviousMatch";
        };
      }
      {
        "context" = "ProjectSearchBar";
        "bindings" = {
          "shift-enter" = "search::SelectPreviousMatch";
        };
      }
    ];
  };
}
