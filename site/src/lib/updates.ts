export type SiteUpdateLink = {
  label: string;
  href: string;
};

export type SiteUpdate = {
  id: string;
  date: string;
  title: string;
  summary: string;
  paragraphs: string[];
  links: SiteUpdateLink[];
};

export const siteUpdates: SiteUpdate[] = [
  {
    id: 'recorded-runner',
    date: '2026-05-25',
    title: 'Recorded command runs become a package',
    summary:
      'The new `run` package keeps long command output compact while saving replayable and queryable artifacts.',
    paragraphs: [
      '`nix run .#run -- <command> ...` executes the command in a PTY, prints a bounded head and tail summary, and writes the complete live stream under `./.ix/run/latest`.',
      'Each session includes `scriptreplay` timing files, an asciinema cast, chunk-level JSONL, line-level JSONL for pandas, and a summary file with duration and exit status.',
      'The live `output.log` and helper scripts let another shell follow a slow build before the command has finished.'
    ],
    links: [
      {
        label: 'run package',
        href: 'https://github.com/indexable-inc/index/tree/main/packages/run'
      },
      {
        label: 'flake package wiring',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/per-system.nix'
      }
    ]
  },
  {
    id: 'fleet-secret-refs',
    date: '2026-05-24',
    title: 'Fleet secret refs become typed plan data',
    summary:
      'ix fleets can now declare secret references once and hand VM modules stable runtime paths.',
    paragraphs: [
      'Fleet specs carry a provider block plus per-secret keys, while modules read `secretRefs` instead of spelling `/run/secrets` paths by hand.',
      'The first documented shape uses a Vaultwarden-style backend for S3 scraper credentials. The generated plan still stays pure JSON, so a future reconciler can materialize files before services start.',
      'This does not put secret bytes in the Nix store. Services still consume runtime files through systemd credentials where the module already supports that pattern.'
    ],
    links: [
      {
        label: 'fleet helper',
        href: 'https://github.com/indexable-inc/index/blob/main/lib/fleet.nix'
      },
      {
        label: 'scraper secret example',
        href: 'https://github.com/indexable-inc/index/blob/main/examples/python-daily-scraper/README.md#s3-output'
      }
    ]
  },
  {
    id: 'site-audio-briefs',
    date: '2026-05-23',
    title: 'Audio briefs land on the site',
    summary:
      'The ix images site now has bite-size update entries and a browser-read audio brief.',
    paragraphs: [
      'Public project notes now live as compact updates with exact repo links close to the text they explain.',
      'The audio controls use browser speech synthesis, so GitHub Pages can keep serving static files while each reader picks an available voice from their device.',
      'The full brief button queues the update feed as one listenable pass for anyone checking the project between tasks.'
    ],
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
  'ix images publishes pre-built OCI images and composable NixOS modules for ix VMs.';

export function updateScript(update: SiteUpdate): string {
  return [update.title, update.summary, ...update.paragraphs].join(' ');
}

export function feedScript(updates: SiteUpdate[]): string {
  const entries = updates.map((update, index) =>
    [
      `Update ${String(index + 1)}. ${update.date}.`,
      updateScript(update)
    ].join(' ')
  );

  return ['ix images update brief.', siteIntro, ...entries].join(' ');
}
