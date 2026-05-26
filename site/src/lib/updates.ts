export type SiteUpdateLink = {
  label: string;
  href: string;
};

export type SiteUpdate = {
  id: string;
  date: string;
  title: string;
  body: string;
  links: SiteUpdateLink[];
};

export const siteUpdates: SiteUpdate[] = [
  {
    id: 'nix-run-site',
    date: '2026-05-26',
    title: '`nix run .#site` previews the page locally',
    body: `The \`site\` package is now a \`symlinkJoin\` of the deploy artifact and a tiny \`miniserve\` wrapper, so \`nix build .#site\` still emits the GitHub Pages tree and \`nix run .#site\` boots a preview at \`http://127.0.0.1:8080/\`.

The wrapper points at a second \`buildNpmSite\` invocation whose \`preBuild\` exports \`BASE_PATH=\`, leaning on SvelteKit's own base-path plumbing instead of rewriting URLs in the server layer. \`buildNpmSite\` stays focused on building.`,
    links: [
      {
        label: 'per-system wiring',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/per-system.nix'
      },
      {
        label: 'build-npm-site',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/build-npm-site.nix'
      }
    ]
  },
  {
    id: 'cargo-unit-per-test',
    date: '2026-05-26',
    title: 'cargo-unit caches Rust tests per `#[test]`',
    body: `\`nix-cargo-unit\` used to build one derivation per test binary; a single flaky case re-ran every test in the file and lost Nix scheduler parallelism. The generated \`tests.<binary>\` attrset now exposes \`all\` (legacy whole-binary behavior) plus \`cases."mod::test_x"\`, one \`runCommand\` per individual test, invoked with \`--exact\`.

Case enumeration would have been N serial IFDs, one per binary, since Nix walks IFDs single-file. They are collapsed into one \`testManifestDrv\` that depends on every test binary and writes per-target \`.list\` and \`.ignored.list\` files. Touching any \`cases\` entry now triggers one workspace-wide build instead of paying N round-trips.

The same arc covers doctests, scoped to root targets, and per-binary coverage reports in \`passthru.coverage\`.`,
    links: [
      {
        label: 'nix-cargo-unit',
        href: 'https://github.com/indexable-inc/index/tree/main/packages/nix-cargo-unit'
      },
      {
        label: 'units template',
        href: 'https://github.com/indexable-inc/index/blob/main/packages/nix-cargo-unit/templates/units.nix.askama'
      }
    ]
  },
  {
    id: 'agents-md-fragments',
    date: '2026-05-25',
    title: '`AGENTS.md` is generated from reusable fragments',
    body: `The contributor guide is no longer a single hand-edited file. It is built from \`agents-md/sections/\` fragments, exposed through \`lib.agentsMd.{sections, profiles, render}\`, and rendered by \`nix run .#agents-md\`. A flake check enforces that the committed \`AGENTS.md\` matches the generated output.

Sibling repos can pull in shared guidance instead of copy-pasting it. The ix repo already publishes its agnostic sections through the same API and this repo imports them at the top of the render pipeline.`,
    links: [
      {
        label: 'agents-md helper',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/agents-md.nix'
      },
      {
        label: 'fragments',
        href: 'https://github.com/indexable-inc/index/tree/main/agents-md/sections'
      }
    ]
  },
  {
    id: 'run-recorder',
    date: '2026-05-25',
    title: '`nix run .#run` records command sessions',
    body: `\`nix run .#run -- <command> ...\` executes the command in a PTY, prints a bounded head/tail summary to the terminal, and writes the full live stream under \`./.ix/run/latest/\`.

Each session captures \`scriptreplay\` timing files, an asciinema cast, chunk-level JSONL, line-level JSONL ready for pandas, and a summary manifest with duration and exit status. A second shell can \`tail -f output.log\` while the original command is still running, which is useful for slow Nix builds and long test suites.`,
    links: [
      {
        label: 'run package',
        href: 'https://github.com/indexable-inc/index/tree/main/packages/run'
      }
    ]
  },
  {
    id: 'ix-dev-diagnose',
    date: '2026-05-25',
    title: '`ix-dev-diagnose` probes ix.dev reachability',
    body: `\`nix run .#ix-dev-diagnose\` reaches \`https://ix.dev/\` from the caller's network path, prints \`success\` or \`failure\`, and writes one JSON report capturing system resolver answers, per-address TCP and TLS results, parsed certificate issuers and fingerprints, native and Mozilla-root verification outcomes, response headers, and a bounded body sample.

Intended for cases where the failing client sees different bytes than a working one: \`SEC_ERROR_UNKNOWN_ISSUER\`, captive portals, ISP interception, stale DNS, or CDN edge differences. Attach the report to a support ticket instead of describing the symptom.`,
    links: [
      {
        label: 'diagnostic package',
        href: 'https://github.com/indexable-inc/index/tree/main/packages/ix-dev-diagnose'
      }
    ]
  },
  {
    id: 'observability-stack',
    date: '2026-05-23',
    title: 'Self-hosted OpenTelemetry stack module',
    body: `\`modules/services/observability\` now wires a complete self-hosted stack: an OpenTelemetry collector, Tempo for traces, Loki for logs, Mimir or Prometheus for metrics, and Grafana with a generated overview dashboard. The dashboard is defined in Nix through \`dashboards/lib.nix\`, so panels can be composed and reused instead of pasted as JSON blobs.

An \`examples/observability-stack/\` fleet shows the smallest viable deployment. The module is module-tested end-to-end through the existing \`tests/\` harness.`,
    links: [
      {
        label: 'observability module',
        href: 'https://github.com/indexable-inc/index/tree/main/modules/services/observability'
      },
      {
        label: 'example fleet',
        href: 'https://github.com/indexable-inc/index/tree/main/examples/observability-stack'
      }
    ]
  }
];

export const siteUrl = 'https://indexable-inc.github.io/index/';
export const siteFeedUrl = `${siteUrl}feed.xml`;
export const siteIntro =
  'Pre-built OCI images and composable NixOS modules for ix VMs.';

export function plainText(markdown: string): string {
  return markdown
    .replace(/`([^`]+)`/g, '$1')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/\s+/g, ' ')
    .trim();
}

export function updateScript(update: SiteUpdate): string {
  return `${update.title}. ${plainText(update.body)}`;
}
