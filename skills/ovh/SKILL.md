---
name: ovh
description: Manage OVH dedicated servers via their REST API (reboot, status, tasks)
---

Manage OVH dedicated servers using the OVH API. Covers hard reboot, server info, listing servers, and task tracking.

## API reference

Base URL: `https://api.us.ovhcloud.com/1.0`

Full console docs: https://api.us.ovhcloud.com/console/?section=/dedicated/server&branch=v1

Full API schema (all endpoints, models, parameters): `dedicated-server.json` in this skill directory.
Read `dedicated-server.json` when you need parameter details, response models, or to discover endpoints not listed below.

### Key endpoints

| Method | Path                                                   | Description                                           |
| ------ | ------------------------------------------------------ | ----------------------------------------------------- |
| GET    | `/dedicated/server`                                    | List all dedicated server service names               |
| GET    | `/dedicated/server/{serviceName}`                      | Get server properties (name, state, IP, OS, etc.)     |
| POST   | `/dedicated/server/{serviceName}/reboot`               | **Hard reboot** the server                            |
| GET    | `/dedicated/server/{serviceName}/task`                 | List server tasks (filter by `function` and `status`) |
| GET    | `/dedicated/server/{serviceName}/task/{taskId}`        | Get task status                                       |
| POST   | `/dedicated/server/{serviceName}/task/{taskId}/cancel` | Cancel a task                                         |
| GET    | `/dedicated/server/{serviceName}/intervention`         | Technical intervention history                        |
| GET    | `/dedicated/server/{serviceName}/intervention/{id}`    | Get intervention details                              |
| GET    | `/dedicated/server/{serviceName}/plannedIntervention`  | Planned interventions for the server                  |
| GET    | `/dedicated/server/{serviceName}/plannedIntervention/{id}` | Get planned intervention details                  |
| GET    | `/dedicated/server/{serviceName}/features/ipmi`        | IPMI status                                           |
| POST   | `/dedicated/server/{serviceName}/features/ipmi/access` | Request KVM IPMI access                               |
| POST   | `/dedicated/server/{serviceName}/features/ipmi/resetInterface` | Reset KVM IPMI interface                      |
| GET    | `/dedicated/server/{serviceName}/ongoing`              | What is ongoing on this server                        |
| GET    | `/dedicated/server/{serviceName}/specifications/hardware` | Hardware info                                      |

### Auth signature

Every request requires these headers:

- `X-Ovh-Application: {application_key}`
- `X-Ovh-Timestamp: {unix_timestamp}` (from `GET /auth/time`)
- `X-Ovh-Signature: $1${sha1}` where sha1 is of `{application_secret}+{consumer_key}+{METHOD}+{full_url}+{body}+{timestamp}`
- `X-Ovh-Consumer: {consumer_key}`

## Credentials

Stored in Vaultwarden folder `ix-infra`, item `OVH US API`:

```
rbw get --folder ix-infra --field "Application Key" "OVH US API"
rbw get --folder ix-infra --field "Application Secret" "OVH US API"
rbw get --folder ix-infra --field "Consumer Key" "OVH US API"
```

### Required token permissions

The current token may only have `/ip/*` permissions. For server management, these additional permissions are needed:

- `GET /dedicated/server/*`
- `POST /dedicated/server/*`

If the token lacks these, the user must create a new token at https://api.us.ovhcloud.com/createToken/ with the required permissions, then update the Vaultwarden item.

## Inventory

| Internal name | OVH service name               | Type    | IP             | OVH model  |
| ------------- | ------------------------------ | ------- | -------------- | ---------- |
| hil-compute-1 | ns1032148.ip-15-204-111.us     | Compute | 15.204.111.75  | SCALE-a5   |
| hil-stor-1    | ns1024928.ip-15-204-106.us     | Storage | 15.204.106.118 | HGR-STOR-1 |

## Helper script

`scripts/infra/ovh/api.sh` — generic OVH API caller with signature auth.

```bash
# List servers
./scripts/infra/ovh/api.sh GET /dedicated/server

# Get server info
./scripts/infra/ovh/api.sh GET /dedicated/server/{serviceName}

# Hard reboot
./scripts/infra/ovh/api.sh POST /dedicated/server/{serviceName}/reboot

# Check task status
./scripts/infra/ovh/api.sh GET /dedicated/server/{serviceName}/task
./scripts/infra/ovh/api.sh GET /dedicated/server/{serviceName}/task/{taskId}

# Intervention history
./scripts/infra/ovh/api.sh GET /dedicated/server/{serviceName}/intervention
./scripts/infra/ovh/api.sh GET /dedicated/server/{serviceName}/intervention/{interventionId}
```

## Steps for a hard reboot

1. **Confirm with the user** before rebooting — this is a destructive operation.

2. **Issue the reboot** using the OVH service name from the inventory:

   ```bash
   ./scripts/infra/ovh/api.sh POST /dedicated/server/{serviceName}/reboot
   ```

3. **Track the task** — the reboot returns a task ID. Poll it:

   ```bash
   ./scripts/infra/ovh/api.sh GET /dedicated/server/{serviceName}/task/{taskId}
   ```

   Status values: `init`, `doing`, `done`, `error`.

4. **Verify** the server comes back by SSHing or pinging.
