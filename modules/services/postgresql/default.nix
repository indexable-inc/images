# PostgreSQL 18 with performance-tuned defaults for AMD EPYC Gen 5 (Zen 5).
{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkDefault
    mkEnableOption
    mkIf
    mkPackageOption
    mkOption
    types
    ;
  cfg = config.services.ix-postgresql;
  pgIsReady = lib.getExe' config.services.postgresql.package "pg_isready";
in {
  options.services.ix-postgresql = {
    enable = mkEnableOption "PostgreSQL 18";

    port = mkOption {
      type = types.port;
      default = 5432;
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the PostgreSQL port in the firewall.";
    };

    dataDir = mkOption {
      type = types.str;
      default = "/var/lib/postgresql/18";
    };

    package = mkPackageOption pkgs "postgresql_18_ix" {
      extraDescription = ''
        PostgreSQL package to run. The default includes the trusted `uint128`
        extension so non-superuser database owners can create it in migrations.
      '';
    };

    sharedBuffersMiB = mkOption {
      type = types.ints.positive;
      default = 256;
      description = ''
        Postgres `shared_buffers` in MiB. Drives both the rendered
        server setting and the kernel's `vm.nr_hugepages` reservation
        so the two stay coherent. Override this single value to
        resize the buffer cache: the matching hugepage pool moves
        with it.
      '';
    };
  };

  config = mkIf cfg.enable {
    ix.networking.portClaims.ix-postgresql = {
      protocol = "tcp";
      inherit (cfg) port;
      description = "PostgreSQL";
    };

    networking.firewall.allowedTCPPorts = lib.optional cfg.openFirewall cfg.port;

    ix.healthChecks.ix-postgresql = {
      from = "guest";
      # `pg_isready` is a real readiness probe: returns 0 only when postmaster
      # accepts connections, not merely when systemd marked the unit active.
      # Catches "starting up", "rejecting connections", and crashed-but-not-yet-
      # failed states that `systemctl is-active` misses.
      description = "PostgreSQL accepts connections";
      command = [
        pgIsReady
        "--quiet"
        "--host"
        "/run/postgresql"
        "--port"
        (toString cfg.port)
      ];
    };

    # `services.postgresql.settings.huge_pages = "on"` (below) makes
    # postmaster refuse to start without a sufficient pool of 2 MiB
    # hugepages. Reserve them at boot, sized from `sharedBuffersMiB`
    # plus headroom for `wal_buffers` (64 MiB) and per-connection
    # mappings. Pages reserved here are locked out of the regular
    # page cache for the lifetime of the boot.
    #
    # `vm.swappiness = 1` keeps the kernel from paging shared_buffers
    # out under memory pressure: PG's own buffer manager tracks hot
    # pages better than the kernel, and swapping a hot page turns a
    # sub-millisecond hit into a disk read. PG community guidance
    # for dedicated DB hosts is 1-10.
    #
    # `vm.dirty_{background_,}ratio` bound the dirty-page cache so a
    # checkpoint flush does not stall client I/O behind tens of GiB
    # of accumulated dirty WAL. The values match the conservative
    # end of the PG community recommendation for WAL-heavy NVMe.
    boot.kernel.sysctl = {
      "vm.nr_hugepages" = (cfg.sharedBuffersMiB / 2) + 32;
      "vm.swappiness" = 1;
      "vm.dirty_background_ratio" = 5;
      "vm.dirty_ratio" = 10;
    };

    services.postgresql = {
      enable = true;
      inherit (cfg) package;
      inherit (cfg) dataDir port;
      enableJIT = true;
      # Tuned defaults for a dedicated VM. Override any of these by setting
      # `services.postgresql.settings.<key>` in the same module; the user
      # assignment wins over `mkDefault`.
      settings = lib.mapAttrs (_: mkDefault) {
        # connections
        listen_addresses = "*";
        max_connections = "200";

        # memory
        shared_buffers = "${toString cfg.sharedBuffersMiB}MB";
        effective_cache_size = "768MB";
        work_mem = "4MB";
        maintenance_work_mem = "128MB";

        # WAL
        wal_buffers = "64MB";
        max_wal_size = "4GB";
        min_wal_size = "512MB";
        wal_level = "replica";
        wal_compression = "zstd";
        checkpoint_completion_target = "0.9";

        # async I/O (PG 18): worker parallelizes checksum/memcpy across processes
        io_method = "worker";
        io_workers = "8";

        # query planner
        random_page_cost = "1.1"; # NVMe
        effective_io_concurrency = "200"; # NVMe
        maintenance_io_concurrency = "200"; # NVMe: VACUUM, CREATE INDEX
        default_statistics_target = "100";

        # parallelism
        max_worker_processes = "8";
        max_parallel_workers_per_gather = "4";
        max_parallel_workers = "8";
        max_parallel_maintenance_workers = "4";

        # logging
        log_min_duration_statement = "1000"; # log queries over 1s

        # EPYC supports 2MB and 1GB huge pages. Hugepage pool is sized
        # from `sharedBuffersMiB` in the `boot.kernel.sysctl` block above.
        huge_pages = "on";

        # JIT
        jit = "on";
      };
    };
  };
}
