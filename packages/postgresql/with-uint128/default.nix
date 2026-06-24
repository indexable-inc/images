{
  pkgs,
  postgresql ? pkgs.postgresql_18,
  postgresql_18_uint128 ? pkgs.postgresql_18_uint128,
}:

(postgresql.withPackages (_: [ postgresql_18_uint128 ])).overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    uint128Extension = postgresql_18_uint128;
  };
})
