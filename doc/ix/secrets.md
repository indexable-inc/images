# secrets

Give a VM a secret in two steps: **declare** it once in your account store with
`ix secret set NAME`, then **attach** it when you create a VM with `ix new
--secret NAME` (or mark it a default). At boot ix **materializes** the attached
secret inside the guest, as an environment variable by default or as a file, and
your service consumes it from there. Values are write-only: they go in through
`set` and are never shown back. For every flag, run `ix secret
--help` and `ix new --help` rather than copying flags from here.

## The model: declare, attach, materialize

1. **Declare.** `ix secret set NAME` stores or rotates one secret in your account
   store, encrypted. The value comes from a hidden prompt, stdin, or `--value-file
   PATH`: never from the command line. The name doubles as the
   env var name when delivered as an env var.
2. **Attach.** A secret is only delivered to VMs you attach it to. Attach per VM
   at create time with `ix new --secret NAME` (repeat for several), or `ix run
   --secret NAME`. Or mark it a default with
   `ix secret set NAME --default` so it attaches to every new VM automatically;
   opt a VM out with `ix new --no-default-secrets`.
3. **Materialize.** At boot, ix injects each attached secret into the guest, as an
   env var by default, or as a file when you set `--file`.

## Example: a secret as an env var

```
ix secret set GH_TOKEN            # paste the value at the hidden prompt
ix new ix/base:latest --secret GH_TOKEN
```

Inside the guest the value is the environment variable `GH_TOKEN`, available to
the image command (the env var name is the secret name).

## Example: a secret as a file

Use `--file GUEST_PATH` to land the value as a file instead of an env var; pair it
with `--owner` (guest unix user, defaults to root) and `--mode` (octal bits,
defaults to 0600):

```
ix secret set NPM_TOKEN --file .npmrc --owner app --mode 0440
ix new ix/base:latest --secret NPM_TOKEN
```

The guest gets `.npmrc` owned by `app` with mode 0440. `--owner` and `--mode`
require `--file`.

## Manage and verify

- `ix secret ls` lists names and metadata only, never values.
- `ix secret check NAME ...` exits non-zero listing any missing names, so deploy
  tooling can fail fast before doing work.
- `ix secret rm NAME` deletes a stored secret. VMs that already attached it keep
  their copy.

## Rotation

Re-run `ix secret set NAME` with the new value. The store is updated, but VMs that
already attached the old value keep it: attached copies are materialized at boot
and persist until the VM is recreated. To pick up a rotated
value, recreate the VM (delete and `ix new` again, or use a fleet `up`/`replace`).

## Fleet plans

A [fleet plan](../ix-fleet/overview.md) references secrets by name, never by
value: plaintext stays in your account store, only the names live in the plan
(`packages/ix-fleet/src/ix_fleet/__init__.py:85-89`). Each node lists `secrets:
[NAME, ...]` and may set `noDefaultSecrets: true`, the exact equivalents of `ix
new --secret NAME` and `--no-default-secrets` (`__init__.py:89-92`). Before any
work, the CLI verifies every referenced secret exists in the store, mirroring `ix
secret check`, and tells you to `ix secret set NAME` if one is missing
(`__init__.py:374-394`). The plan's optional `secrets.provider` defaults to type
`runtime-directory` with `mountRoot` `/run/secrets`, so file-mounted secrets land
under `/run/secrets/<name>` (`__init__.py:124-128`).

## How platform services are wired (not your path)

The platform's own host services do not use the account store above. Their
secrets are encrypted per-machine with the host's TPM2 chip and stored at
`/etc/credstore.encrypted/<name>`; systemd decrypts them at service start via
systemd-creds into the service's `$CREDENTIALS_DIRECTORY`. The canonical names map
to a Vaultwarden-backed store for provisioning. This is internal
platform plumbing; your path is `ix secret` plus fleet `secrets` above.

## See also

- [cli.md](cli.md): the `ix` CLI surface.
- [fleet.md](fleet.md) and [ix-fleet overview](../ix-fleet/overview.md): declarative fleet plans.
- [services.md](services.md): running and wiring your service in the guest.
- [networking.md](networking.md): VM connectivity and ingress.
