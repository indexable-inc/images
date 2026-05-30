#!/usr/bin/env bun
// Better Stack Uptime API v2 client. Run with bun (no build step):
//   BETTERSTACK_API_TOKEN=… bun bs.ts <command> [args] [--flags]
//
// Token resolution order: --token <t>, $BETTERSTACK_API_TOKEN, then
//   `rbw get --folder ix-infra --field token "Better Stack Uptime API"`.
//
// The API follows JSON:API: every resource is { id, type, attributes, relationships }.
// Docs: https://betterstack.com/docs/uptime/api/

const BASE = "https://uptime.betterstack.com/api/v2";

type Json = Record<string, any>;
type Resource = { id: string; type: string; attributes: Json; relationships?: Json };

// --- argv parsing -----------------------------------------------------------

const argv = process.argv.slice(2);
const positional: string[] = [];
const flags: Record<string, string | boolean> = {};
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a.startsWith("--")) {
    const key = a.slice(2);
    const next = argv[i + 1];
    if (next !== undefined && !next.startsWith("--")) {
      flags[key] = next;
      i++;
    } else {
      flags[key] = true;
    }
  } else {
    positional.push(a);
  }
}
const asJson = flags.json === true;

// --- token ------------------------------------------------------------------

function resolveToken(): string {
  if (typeof flags.token === "string") return flags.token;
  if (process.env.BETTERSTACK_API_TOKEN) return process.env.BETTERSTACK_API_TOKEN;
  // Fall back to Vaultwarden (shared ix-infra store). Requires `rbw unlock`.
  try {
    const out = Bun.spawnSync([
      "rbw", "get", "--folder", "ix-infra", "--field", "token", "Better Stack Uptime API",
    ]);
    const tok = new TextDecoder().decode(out.stdout).trim();
    if (tok) return tok;
  } catch {}
  console.error(
    "No token. Pass --token, set BETTERSTACK_API_TOKEN, or store it in Vaultwarden\n" +
      '(ix-infra / "Better Stack Uptime API" / field "token") and run `rbw unlock`.',
  );
  process.exit(1);
}

// --- HTTP -------------------------------------------------------------------

async function req(method: string, path: string, body?: Json): Promise<Json> {
  const url = path.startsWith("http") ? path : `${BASE}${path.startsWith("/") ? path : `/${path}`}`;
  const res = await fetch(url, {
    method,
    headers: {
      Authorization: `Bearer ${resolveToken()}`,
      "Content-Type": "application/json",
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  const data = text ? JSON.parse(text) : {};
  if (!res.ok) {
    console.error(`HTTP ${res.status} ${method} ${url}`);
    console.error(JSON.stringify(data, null, 2));
    process.exit(1);
  }
  return data;
}

// GET a collection, optionally following pagination (.pagination.next) with --all.
async function getList(path: string): Promise<Resource[]> {
  const items: Resource[] = [];
  let url: string | null = path;
  do {
    const page: Json = await req("GET", url);
    items.push(...(page.data ?? []));
    url = flags.all === true ? page.pagination?.next ?? null : null;
  } while (url);
  return items;
}

// --- rendering --------------------------------------------------------------

function table(rows: Record<string, any>[], cols: string[]): void {
  if (rows.length === 0) {
    console.log("(none)");
    return;
  }
  const widths = cols.map((c) =>
    Math.max(c.length, ...rows.map((r) => String(r[c] ?? "").length)),
  );
  const fmt = (cells: any[]) =>
    cells.map((v, i) => String(v ?? "").padEnd(widths[i])).join("  ");
  console.log(fmt(cols));
  console.log(widths.map((w) => "-".repeat(w)).join("  "));
  for (const r of rows) console.log(fmt(cols.map((c) => r[c])));
}

function out(value: any, render?: () => void): void {
  if (asJson || !render) console.log(JSON.stringify(value, null, 2));
  else render();
}

// --- commands ---------------------------------------------------------------

const [cmd, arg] = positional;

const HELP = `Better Stack Uptime API CLI (bun bs.ts <command>)

Monitors:
  monitors [--status up|down|paused] [--all]   List monitors
  monitor <id>                                  Show one monitor
  pause <id> | unpause <id>                      Toggle a monitor

Heartbeats:
  heartbeats [--all]                            List heartbeats
  heartbeat <id>                                Show one heartbeat

Incidents:
  incidents [--all]                             List incidents
  incident <id>                                 Show one incident
  ack <id>                                       Acknowledge an incident
  resolve <id>                                   Resolve an incident

Status pages:
  status-pages [--all]                          List status pages

Raw escape hatch (any v2 endpoint):
  get <path>                                     e.g. get /monitors?page=2
  post <path> <json>                            e.g. post /monitors '{"...":...}'
  patch <path> <json>
  delete <path>

Global flags: --json (raw JSON), --all (follow pagination), --token <t>`;

switch (cmd) {
  case undefined:
  case "help":
  case "-h":
  case "--help":
    console.log(HELP);
    break;

  case "monitors": {
    let list = await getList("/monitors");
    if (typeof flags.status === "string")
      list = list.filter((m) => m.attributes.status === flags.status);
    out(list, () =>
      table(
        list.map((m) => ({
          id: m.id,
          name: m.attributes.pronounceable_name || m.attributes.url,
          type: m.attributes.monitor_type,
          status: m.attributes.status,
          freq: `${m.attributes.check_frequency}s`,
        })),
        ["id", "name", "type", "status", "freq"],
      ),
    );
    break;
  }

  case "monitor":
    out((await req("GET", `/monitors/${arg}`)).data);
    break;

  case "pause":
    out((await req("PATCH", `/monitors/${arg}`, { paused: true })).data);
    break;

  case "unpause":
    out((await req("PATCH", `/monitors/${arg}`, { paused: false })).data);
    break;

  case "heartbeats": {
    const list = await getList("/heartbeats");
    out(list, () =>
      table(
        list.map((h) => ({
          id: h.id,
          name: h.attributes.name,
          status: h.attributes.status,
          period: `${h.attributes.period}s`,
          grace: `${h.attributes.grace}s`,
        })),
        ["id", "name", "status", "period", "grace"],
      ),
    );
    break;
  }

  case "heartbeat":
    out((await req("GET", `/heartbeats/${arg}`)).data);
    break;

  case "incidents": {
    const list = await getList("/incidents");
    out(list, () =>
      table(
        list.map((i) => ({
          id: i.id,
          name: i.attributes.name,
          cause: i.attributes.cause,
          status: i.attributes.status,
          started: i.attributes.started_at,
        })),
        ["id", "name", "cause", "status", "started"],
      ),
    );
    break;
  }

  case "incident":
    out((await req("GET", `/incidents/${arg}`)).data);
    break;

  case "ack":
    out((await req("POST", `/incidents/${arg}/acknowledge`)).data);
    break;

  case "resolve":
    out((await req("POST", `/incidents/${arg}/resolve`)).data);
    break;

  case "status-pages": {
    const list = await getList("/status-pages");
    out(list, () =>
      table(
        list.map((s) => ({
          id: s.id,
          name: s.attributes.company_name || s.attributes.subdomain,
          url: s.attributes.custom_domain || `${s.attributes.subdomain}.betteruptime.com`,
        })),
        ["id", "name", "url"],
      ),
    );
    break;
  }

  // Raw escape hatch for any endpoint not wrapped above.
  case "get":
    out(await req("GET", arg));
    break;
  case "post":
    out(await req("POST", arg, positional[2] ? JSON.parse(positional[2]) : undefined));
    break;
  case "patch":
    out(await req("PATCH", arg, positional[2] ? JSON.parse(positional[2]) : undefined));
    break;
  case "delete":
    out(await req("DELETE", arg));
    break;

  default:
    console.error(`Unknown command: ${cmd}\n`);
    console.log(HELP);
    process.exit(1);
}
