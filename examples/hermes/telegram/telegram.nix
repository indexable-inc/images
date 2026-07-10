# Telegram-specific deltas on top of the shared hermes-agent
# composition (`ix.hermes.profile`). The toggle bag flips the
# long-poll Telegram platform on; the persona swap is the only option
# override, because a chat companion reads differently from a terminal
# operator.
{ix, ...}: {
  # The shared composition reads this bag; `telegram = true` wires the
  # Telegram env file into the daemon. The platform itself activates
  # when TELEGRAM_BOT_TOKEN is present in /run/secrets/hermes.env, so a
  # freshly booted VM without the token still comes up healthy.
  _module.args.hermes = {
    telegram = true;
  };

  # Chat-tuned SOUL.md: short messages, no markdown walls, Telegram
  # formatting quirks. The shared composition binds its operator persona
  # with mkDefault, so a plain assignment here wins.
  services.hermes-agent.documents."SOUL.md" = ix.hermes.documents.telegram."SOUL.md";
}
