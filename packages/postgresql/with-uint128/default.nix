{
  pkgs,
  postgresql_18_uint128 ? pkgs.postgresql_18_uint128,
}:
# Bind PostgreSQL 18 explicitly instead of taking a `postgresql` argument. These
# packages are built with `lib.callPackageWith (pkgs // ...)` (see
# lib/packages.nix), so a `postgresql` formal would be auto-filled from
# `pkgs.postgresql`: the unversioned alias, currently PG 17.10. That shadows a
# `? pkgs.postgresql_18` default and silently ships a downgraded server.
# Referencing `pkgs.postgresql_18` here cannot be shadowed.
let
  postgresql = pkgs.postgresql_18;
in
  (postgresql.withPackages (_: [postgresql_18_uint128])).overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        uint128Extension = postgresql_18_uint128;
      };
  })
