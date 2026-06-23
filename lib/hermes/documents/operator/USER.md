# USER

Replace this file with notes about yourself, your projects, and the way you want the agent to talk to you. Hermes injects it into the system prompt on every session, so it is the cheapest place to land long-running context.

A few shapes that work well:

- Who you are and what you do, in one sentence. The agent picks up tone from this.
- Projects you are likely to ask about. List the repo paths, the language, the deploy target.
- House rules. "Never amend a commit on `main`." "Always run `nix run .#lint` before pushing." "Open a draft PR before pushing the first commit." Hermes respects these the same way it respects `AGENTS.md` files.
- Preferences that would otherwise burn a turn to discover. Editor, package manager, shell, time zone.

Keep it scannable. The agent reads this file every session and the bytes you spend here come out of your context window.
