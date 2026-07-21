/**
TigerBeetle's documented deployment contract as data, one owner for every
repo that runs a replica (today: ix's billing-ledger). Values come from the
vendor docs, not from any deployment's measurements; a consumer that wants
different numbers is diverging from the vendor and should say so at its own
call site.

- `minRamGiBPerReplica` / `recommendedCacheRamGiB`: hardware floor and the
  recommended cache allocation range
  (https://docs.tigerbeetle.com/operating/hardware/). TigerBeetle allocates
  its whole footprint statically at startup and never shrinks, so a memory
  limit below the live footprint is permanent reclaim thrash by
  construction (ix#8020), never pressure relief.
- `referenceCacheGrid`: the `--cache-grid` value the vendor's reference
  systemd unit pins explicitly
  (https://docs.tigerbeetle.com/operating/deploying/systemd/). Pass it (or
  a deliberate override) so the static footprint is chosen, not floating on
  a binary default.
- `capabilities`: the reference unit ships
  `AmbientCapabilities=CAP_IPC_LOCK` so TigerBeetle can mlock everything it
  allocates (io_uring setup, and keeping swap from silently bypassing its
  storage fault model). CAP_IPC_LOCK also exempts mlock from
  RLIMIT_MEMLOCK, so no LimitMEMLOCK override is needed alongside it.
*/
{
  minRamGiBPerReplica = 6;
  recommendedCacheRamGiB = {
    low = 16;
    high = 32;
  };
  referenceCacheGrid = "1GiB";
  capabilities = ["CAP_IPC_LOCK"];
}
