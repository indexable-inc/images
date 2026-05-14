# PostgreSQL 18 with performance-tuned defaults for AMD EPYC Gen 5 (Zen 5).
{
  config,
  ix,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkDefault
    mkEnableOption
    mkIf
    mkOption
    types
    ;
  cfg = config.services.ix-postgresql;
in
{
  options.services.ix-postgresql = {
    enable = mkEnableOption "PostgreSQL 18";

    port = mkOption {
      type = types.port;
      default = 5432;
    };

    dataDir = mkOption {
      type = types.str;
      default = "/var/lib/postgresql/18";
    };
  };

  config = mkIf cfg.enable {
    networking.firewall.allowedTCPPorts = [ cfg.port ];

    services.postgresql = {
      enable = true;
      package = pkgs.postgresql_18;
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
        shared_buffers = "256MB";
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

        # EPYC supports 2MB and 1GB huge pages
        huge_pages = "on";

        # JIT
        jit = "on";
      };
    };
  };
}
