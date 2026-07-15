# User Sources

This directory holds user-owned source material that other repositories consume
through `index.lib.users.<user>.<repo>`.

The `index.lib.users` surface discovers every directory under `users/`, so adding
`users/<another-user>/<repo>` exposes the same source-record shape without a
library change.

Repository-local bootstrap files should stay in the consuming repository when
tools discover them by fixed path. Larger authored inputs, such as agent
instruction fragments and skills, can live here and be exposed as composable
source records.
