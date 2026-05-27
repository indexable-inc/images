## Reply shape

These rules cover answers written back to the user in chat, PR comments, issue
replies, and review notes.

Lead with the highest-impact concrete fact. No preamble about what you are
about to inspect, no filler, no delayed setup.

When an answer depends on repo evidence, include that evidence even for quick
factual answers. A fast `rg` or `Read` hit is a reason to cite the owner
snippet, not a reason to skip it. Prefer the file that directly stores or
defines the answer; fall back to a nearby owner snippet only when the direct
file is unavailable or would expose a secret the user has not asked to inspect.

For codebase questions about where something lives, how it works, what reads
what, why a behavior happens, or what value is configured: lead with a one-line
answer, then the repo-relative path, then a minimal owner snippet, then the
consequence in plain English. Skip "I'll inspect..." unless you are about to
do a longer investigation that benefits from a stated plan.

Avoid inline file paths inside prose unless the path is the answer. Use
descriptive labels and a reference list when there are several paths. This rule
never overrides the snippet requirement.

Use numbered steps for sequences, bullets for parallel facts, and short
paragraphs for explanations. No decorative emoji unless the user asks. No
em dashes and no double-hyphen substitutes in human prose; split the sentence
or change the punctuation.

Avoid rhetorical tics that pattern-match as generated prose: balanced "X, not
Y" contrasts, three-part lists for cadence rather than content, "I'll happily
help with..." openers.

Do not hard-wrap PR descriptions, issue bodies, review comments, or any
Markdown sent to a hosted renderer. Let the renderer reflow paragraphs.
