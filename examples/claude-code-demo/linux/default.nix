{
  ix,
  pkgs,
  lib,
}:
let
  fs = lib.fileset;

  demoSiteSrc = fs.toSource {
    root = ./site;
    fileset = fs.unions [
      ./site/index.html
      ./site/package.json
      ./site/package-lock.json
      ./site/eslint.config.js
      ./site/tsconfig.json
      ./site/src/App.svelte
      ./site/src
      ./site/vite.config.js
    ];
  };

  # site/src/lib/vm-config.json is the single source of truth for the demo's
  # advertised hardware and billing rates. The Svelte UI imports it directly;
  # the Nushell stats writer below interpolates the same values at build time.
  vmConfig = builtins.fromJSON (builtins.readFile ./site/src/lib/vm-config.json);
  vmServer = vmConfig.server;
  vmBilling = vmConfig.billing;

  demoSite = ix.buildNpmSite pkgs {
    pname = "claude-code-demo-site";
    version = "0.1.0";
    src = demoSiteSrc;
  };

  writeStats = ix.writeNushellApplication pkgs {
    name = "claude-code-demo-write-stats";
    runtimeInputs = [ pkgs.coreutils ];
    text = ''
      def read-cpu [] {
        let values = (open --raw /proc/stat | lines | first | split row " " | where $it != "")
        let nums = ($values | skip 1 | each { into int })
        {
          total: ($nums | math sum)
          idle: (($nums | get 3) + ($nums | get 4))
        }
      }

      def round-to [places: int] {
        let factor = (10 ** $places)
        ($in * $factor | math round) / $factor
      }

      def main [] {
        let out_dir = "/run/claude-code-demo"
        mkdir $out_dir

        let cpu_a = (read-cpu)
        sleep 200ms
        let cpu_b = (read-cpu)
        let total_delta = ($cpu_b.total - $cpu_a.total)
        let idle_delta = ($cpu_b.idle - $cpu_a.idle)
        let cpu_percent = if $total_delta <= 0 {
          0.0
        } else {
          (($total_delta - $idle_delta) / $total_delta * 100)
        }

        let mem = (
          open --raw /proc/meminfo
          | lines
          | parse "{key}: {value} kB"
          | reduce -f {} {|row, acc| $acc | insert $row.key ($row.value | str trim | into int)}
        )
        let mem_used_bytes = (($mem.MemTotal - $mem.MemAvailable) * 1024)
        let disk_used_bytes = (^df -B1 --output=used / | lines | get 1 | str trim | into int)

        let cpu_total_cores = ${toString vmServer.vcpu}
        let mem_total_bytes = (${toString vmServer.memoryGiB} * 1024 * 1024 * 1024)
        let disk_total_bytes = (${toString vmServer.storageTiB} * 1024 * 1024 * 1024 * 1024)
        let cpu_used_cores = ($cpu_total_cores * $cpu_percent / 100)
        let memory_used_gib = ($mem_used_bytes / 1024 / 1024 / 1024)
        let disk_used_tib = ($disk_used_bytes / 1024 / 1024 / 1024 / 1024)

        let stats = {
          generatedAt: (date now | date to-timezone UTC | format date "%Y-%m-%dT%H:%M:%SZ")
          cpu: {
            usedCores: ($cpu_used_cores | round-to 4)
            totalCores: $cpu_total_cores
            percent: ($cpu_percent | round-to 4)
          }
          memory: {
            usedBytes: $mem_used_bytes
            totalBytes: $mem_total_bytes
            percent: (($mem_used_bytes / $mem_total_bytes * 100) | round-to 4)
          }
          disk: {
            usedBytes: $disk_used_bytes
            totalBytes: $disk_total_bytes
            percent: (($disk_used_bytes / $disk_total_bytes * 100) | round-to 6)
          }
          costPerSecondUsd: (
            ($cpu_used_cores * (${toString vmBilling.cpuUsdPerVcpuMonth} / (30 * 24 * 60 * 60)))
            + ($memory_used_gib * ((${toString vmBilling.memoryUsdPerGibHour} / (60 * 60)) * ${toString vmBilling.marginMultiplier}))
            + ($disk_used_tib * ((${toString vmBilling.storageUsdPerTibHour} / (60 * 60)) * ${toString vmBilling.marginMultiplier}))
          )
        }

        let tmp = (^mktemp $"($out_dir)/stats.XXXXXX" | str trim)
        $stats | to json | save --force $tmp
        ^chmod 0644 $tmp
        mv --force $tmp $"($out_dir)/stats.json"
      }
    '';
  };

  statsLoop = ix.writeNushellApplication pkgs {
    name = "claude-code-demo-stats-loop";
    runtimeInputs = [
      pkgs.coreutils
      writeStats
    ];
    text = ''
      def main [] {
        loop {
          claude-code-demo-write-stats
          sleep 1sec
        }
      }
    '';
  };

  linuxCompileLibraries = [
    pkgs.elfutils
    pkgs.ncurses
    pkgs.openssl
    pkgs.zlib
  ];
  linuxCompileIncludes = lib.concatMapStringsSep " " (path: "-I${path}") (
    lib.splitString ":" (lib.makeSearchPathOutput "dev" "include" linuxCompileLibraries)
  );
  linuxCompileLibraryPath = lib.concatMapStringsSep " " (path: "-L${path}") (
    lib.splitString ":" (lib.makeLibraryPath linuxCompileLibraries)
  );

  compileLinux = ix.writeNushellApplication pkgs {
    name = "compile";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.git
      pkgs.gnumake
      pkgs.systemd
    ];
    text = ''
      $env.PKG_CONFIG_PATH = "${lib.makeSearchPathOutput "dev" "lib/pkgconfig" linuxCompileLibraries}"
      $env.NIX_CFLAGS_COMPILE = "${linuxCompileIncludes}"
      $env.NIX_LDFLAGS = "${linuxCompileLibraryPath}"

      def env-or [name: string, fallback: string] {
        let value = ($env | get --optional $name)
        if $value == null or ($value | is-empty) {
          $fallback
        } else {
          $value
        }
      }

      def run-throttled [cpu_quota: string, memory_max: string, command: list<string>] {
        let limits = [
          "-p"
          $"CPUQuota=($cpu_quota)"
          "-p"
          $"MemoryMax=($memory_max)"
        ]

        if ("/run/systemd/private" | path exists) {
          ^systemd-run --quiet --wait --collect ...$limits ...$command
        } else {
          ^$command.0 ...($command | skip 1)
        }
      }

      def source-ready [source_dir: string] {
        [
          "Makefile"
          "kernel"
          "scripts"
          "arch/x86/boot"
        ] | all {|path| ($source_dir | path join $path) | path exists }
      }

      def ensure-source [source_dir: string] {
        if (source-ready $source_dir) {
          return
        }

        if ($source_dir | path exists) {
          rm --recursive --force $source_dir
        }

        mkdir ($source_dir | path dirname)
        ^${lib.getExe pkgs.git} clone --quiet --depth 1 --single-branch https://github.com/torvalds/linux.git $source_dir

        if not (source-ready $source_dir) {
          error make {
            msg: $"Linux source tree bootstrap did not produce a complete checkout at ($source_dir)."
          }
        }
      }

      def main [...targets: string] {
        let source_dir = (env-or LINUX_SOURCE_DIR "/src/linux")
        ensure-source $source_dir

        cd $source_dir

        let cpu_quota = (env-or LINUX_BUILD_CPU_QUOTA "1600%")
        let memory_max = (env-or LINUX_BUILD_MEMORY_MAX "64G")
        let nproc = (^nproc | str trim | into int)
        let default_jobs = ([ $nproc 16 ] | math min | into string)
        let jobs = (env-or LINUX_BUILD_JOBS $default_jobs)

        if not (".config" | path exists) {
          run-throttled $cpu_quota $memory_max ["${lib.getExe pkgs.gnumake}" "defconfig"]
        }

        run-throttled $cpu_quota $memory_max (["${lib.getExe pkgs.gnumake}" $"-j($jobs)"] ++ $targets)
      }
    '';
  };

  linuxBuildPackages = [
    pkgs.bc
    pkgs.bison
    pkgs.elfutils
    pkgs.findutils
    pkgs.flex
    pkgs.gcc
    pkgs.git
    pkgs.gnumake
    pkgs.gnugrep
    pkgs.ncurses
    pkgs.openssl
    pkgs.pahole
    pkgs.perl
    pkgs.pkg-config
    pkgs.python3
    pkgs.rsync
    pkgs.zlib
  ];
in
{
  tags = [ "web" ];
  deployment.l7ProxyPorts = [ 80 ];
  modules = [
    (_: {
      environment.systemPackages = linuxBuildPackages ++ [
        pkgs.btop
        compileLinux
        pkgs.curl
      ];

      services.git-clone = {
        enable = true;
        activation = "timer";
        url = "https://github.com/torvalds/linux.git";
        dest = "/src/linux";
      };

      systemd.services.git-clone.serviceConfig.ExecStartPost =
        "${lib.getExe' pkgs.coreutils "ln"} -sfn ${lib.getExe compileLinux} /src/linux/compile";

      systemd.services.claude-code-demo-stats = {
        description = "Claude Code demo VM stats";
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          Type = "simple";
          RuntimeDirectory = "claude-code-demo";
          RuntimeDirectoryMode = "0755";
          ExecStart = lib.getExe statsLoop;
        };
      };

      services.nginx = {
        enable = true;
        virtualHosts."claude-code-demo" = {
          default = true;
          root = "${demoSite}/share/claude-code-demo-site";
          locations."/stats.json".root = "/run/claude-code-demo";
          locations."/".tryFiles = "$uri $uri/ /index.html";
        };
      };

      networking.firewall.allowedTCPPorts = [ 80 ];
    })
  ];
}
