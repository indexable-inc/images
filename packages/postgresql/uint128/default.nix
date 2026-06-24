{
  ix,
  lib,
  pkgs,
}:
let
  # Bind PostgreSQL 18 explicitly instead of taking a `postgresql` argument. The
  # package framework builds this with `lib.callPackageWith (pkgs // ...)` (see
  # lib/packages.nix), so a `postgresql` formal would be auto-filled from
  # `pkgs.postgresql`: the unversioned alias, currently PG 17.10. That shadows a
  # `? pkgs.postgresql_18` default and builds the extension for the wrong major.
  postgresql = pkgs.postgresql_18;
  postgresqlBuildExtension =
    pkgs.callPackage (pkgs.path + "/pkgs/servers/sql/postgresql/postgresqlBuildExtension.nix")
      {
        inherit postgresql;
      };
  postgresqlTestExtension =
    pkgs.callPackage (pkgs.path + "/pkgs/servers/sql/postgresql/postgresqlTestExtension.nix")
      {
        inherit postgresql;
      };
in
postgresqlBuildExtension (finalAttrs: {
  pname = "uint128";
  version = "1.2.0";

  nativeBuildInputs = [ postgresql.stdenv.cc ];

  src = ix.paths.pgUint128Src;

  # Mark the extension `trusted` so non-superuser DB owners can run
  # `CREATE EXTENSION uint128` during migrations. Safe because pg-uint128 only
  # defines data types and operators, with no filesystem or shell access.
  postInstall = ''
    # shell
    control="$out/share/postgresql/extension/uint128.control"
    if [ ! -f "$control" ]; then
      echo "postgresql-uint128: expected control file at $control" >&2
      exit 1
    fi
    if ! grep -q '^trusted' "$control"; then
      echo "trusted = true" >> "$control"
    fi
  '';

  passthru.tests = {
    extension = postgresqlTestExtension {
      inherit (finalAttrs) finalPackage;
      sql = "CREATE EXTENSION uint128;";
    };
  };

  meta = {
    description = "Unsigned integer types for PostgreSQL";
    homepage = "https://github.com/pg-uint/pg-uint128";
    license = lib.licenses.postgresql;
    maintainers = [ ];
    inherit (postgresql.meta) platforms;
  };
})
