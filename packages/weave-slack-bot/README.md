# weave-slack-bot

`weave-slack-bot` is the standing Slack transport for a named Weave agent. It
holds one Socket Mode connection, durably records accepted Slack events in
Weave, addresses each event to the configured agent, and publishes the agent's
reply to the originating Slack thread.

The process is transport, not the model runtime. Weave owns agent history and
starts or resumes model turns; the bridge can restart without losing accepted
events because its recovery log is stored as Weave facts.

## Runtime contract

The executable requires `SLACK_BOT_OAUTH_TOKEN` and `SLACK_APP_TOKEN`. The app
token needs `connections:write`. Pass the Weave HTTP endpoint, model, named
agent, and system prompt explicitly:

```console
weave-slack-bot \
  --weave-url http://127.0.0.1:4410 \
  --agent slack-bot \
  --model fable \
  --system-prompt /run/config/slack-bot.md
```

Only one instance should own a Slack app unless the deployment provides leader
election. The bridge uses stable Slack and Weave operation identifiers to make
redelivery and crash recovery idempotent.
