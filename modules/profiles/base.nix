# Base runtime profile.
#
# Auto-enabled by `lib/ix-oci-layer.nix`. Ships cross-cutting CLI that should
# be available on every VM for debugging and introspection. Image-specific
# runtime dependencies still belong in the image or service that needs them.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.ix.profiles.base;

  inherit (cfg) shellWorkspace;
  shellWrapper = pkgs.writeTextFile {
    name = "ix-workspace-shell";
    executable = true;
    destination = "/bin/ix-workspace-shell";
    text = ''
      #!${pkgs.runtimeShell}
      set -eu

      workdir="''${IX_WORKDIR:-${shellWorkspace.directory}}"
      mkdir -p -- "$workdir"
      cd -- "$workdir"

      exec ${lib.getExe shellWorkspace.shell} "$@"
    '';
    meta.mainProgram = "ix-workspace-shell";
    passthru.shellPath = "/bin/ix-workspace-shell";
  };
in
{
  options.ix.profiles.base = {
    enable = lib.mkEnableOption "base runtime tools";

    shellWorkspace = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Make interactive root shells enter a writable image workspace before
          starting the configured shell.
        '';
      };

      directory = lib.mkOption {
        type = lib.types.str;
        default = "/work/ix";
        description = "Directory created and entered by the base shell wrapper.";
      };

      shell = lib.mkOption {
        type = lib.types.package;
        default = pkgs.nushell;
        defaultText = lib.literalExpression "pkgs.nushell";
        description = "Shell executed after entering the image workspace.";
      };
    };
  };

  config = lib.mkIf config.ix.profiles.base.enable {
    # Cubic halves cwnd on any loss, so a residential last-mile at
    # 30 ms and a couple percent loss caps a single TCP flow far
    # below the path's real capacity. BBR models bottleneck bandwidth
    # and RTT from delivery-rate measurements and is largely loss-
    # insensitive, which matches every workload here that accepts
    # inbound from arbitrary internet endpoints (Minecraft players,
    # Xpra browser clients, repo fetches via `git-clone`). fq is the
    # qdisc BBR was designed to pace with; BBR without fq leaves
    # bandwidth on the table.
    #
    # If `tcp_bbr` is not present in the running kernel, the sysctl
    # write is a no-op and Cubic stays in place. Per-socket buffer
    # caps (`rmem_max`, `wmem_max`, `tcp_{r,w}mem`) are deliberately
    # left at kernel defaults: a 64 MiB per-socket ceiling is real
    # memory cost on small VMs with many accepted sockets, and the
    # default 4 MiB cap fits the BDP of every workload shipped here.
    boot.kernel.sysctl = {
      "net.ipv4.tcp_congestion_control" = "bbr";
      "net.core.default_qdisc" = "fq";
    };

    environment.systemPackages =
      builtins.attrValues {
        inherit (pkgs)
          bpftrace
          btop
          file
          gdb
          # gnutar, gzip, and zstd ride along so any VM switched once stays
          # switchable: `ix switch --source` streams a tarball through
          # `tar -x -I zstd` inside the guest, and these binaries are not
          # on NixOS' default system PATH.
          gnutar
          gzip
          jq
          lldb
          lsof
          ncdu
          pv
          strace
          tcpdump
          zstd
          ;
      }
      ++ lib.optionals shellWorkspace.enable [
        shellWorkspace.shell
        shellWrapper
      ];

    users.users.root.shell = lib.mkIf shellWorkspace.enable shellWrapper;
  };
}
