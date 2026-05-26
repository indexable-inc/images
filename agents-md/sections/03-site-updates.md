## Site updates

Operator-facing behavior changes should usually get one compact entry in
[`site/src/lib/updates.ts`](site/src/lib/updates.ts). Keep the first sentence
useful when read aloud and put exact links near the detail.

Keep checked-in site builds pure. The site should read text and static assets
from the repo without API keys, paid services, or network side effects. Generated
media, search indexes, catalogs, and similar artifacts belong behind explicit
commands or CI steps that write static outputs before the site build consumes
them.

Prefer a plain text feed before adding richer publication channels. Rich media
feeds need real media files with stable URLs before they are advertised.

