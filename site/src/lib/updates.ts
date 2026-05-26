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
    id: 'ix-dev-diagnose',
    date: '2026-05-25',
    title: 'ix.dev reachability gets a JSON probe',
    body: `\`ix-dev-diagnose\` captures DNS, TLS, certificate, and response-byte clues when ix.dev behaves differently by network.

\`nix run .#ix-dev-diagnose\` probes \`https://ix.dev/\` from the caller's path, prints \`success\` or \`failure\`, and writes one JSON report for sharing with support.

The report records system resolver answers, per-address TCP and TLS results, parsed certificate issuers and fingerprints, native and Mozilla-root verification outcomes, headers, and a bounded response-body sample.

Meant for \`SEC_ERROR_UNKNOWN_ISSUER\`, captive portals, ISP interception, stale DNS, or CDN edge differences where the failing client sees different bytes than a working client.`,
    links: [
      {
        label: 'diagnostic package',
        href: 'https://github.com/indexable-inc/index/tree/main/packages/ix-dev-diagnose'
      },
      {
        label: 'flake wiring',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/per-system.nix'
      }
    ]
  },
  {
    id: 'recorded-runner',
    date: '2026-05-25',
    title: 'Recorded command runs become a package',
    body: `The new \`run\` package keeps long command output compact while saving replayable and queryable artifacts.

\`nix run .#run -- <command> ...\` executes the command in a PTY, prints a bounded head and tail summary, and writes the complete live stream under \`./.ix/run/latest\`.

Each session includes \`scriptreplay\` timing files, an asciinema cast, chunk-level JSONL, line-level JSONL for pandas, and a summary file with duration and exit status. The live \`output.log\` lets another shell follow a slow build before the command finishes.`,
    links: [
      {
        label: 'run package',
        href: 'https://github.com/indexable-inc/index/tree/main/packages/run'
      },
      {
        label: 'flake wiring',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/per-system.nix'
      }
    ]
  },
  {
    id: 'fleet-secret-refs',
    date: '2026-05-24',
    title: 'Fleet secret refs become typed plan data',
    body: `ix fleets can now declare secret references once and hand VM modules stable runtime paths.

Fleet specs carry a provider block plus per-secret keys, while modules read \`secretRefs\` instead of spelling \`/run/secrets\` paths by hand. The first documented shape uses a Vaultwarden-style backend for S3 scraper credentials.

The generated plan stays pure JSON, so a future reconciler can materialize files before services start. Secret bytes never enter the Nix store; services consume runtime files through systemd credentials where the module already supports that pattern.`,
    links: [
      {
        label: 'fleet helper',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/fleet.nix'
      },
      {
        label: 'scraper example',
        href: 'https://github.com/indexable-inc/index/blob/main/examples/python-daily-scraper/README.md#s3-output'
      }
    ]
  },
  {
    id: 'site-audio-briefs',
    date: '2026-05-23',
    title: 'A compact update feed lands on the site',
    body: `Public project notes now live as compact updates with exact repo links close to the text they explain.

GitHub Pages serves static HTML and RSS, so the site stays inspectable without browser-only media controls or runtime services. The feed is meant for quick release notes: one short summary, the operational detail, and links to the owning source.`,
    links: [
      {
        label: 'site source',
        href: 'https://github.com/indexable-inc/index/tree/main/site'
      },
      {
        label: 'contributor note',
        href: 'https://github.com/indexable-inc/index/blob/main/AGENTS.md#site-updates'
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
