// Kimi K3 via Moonshot's OpenAI-compatible endpoint. pi 0.80.3's built-in
// moonshotai provider stops at K2.x, so register a custom one (#3476).
// K3 quirks (https://platform.kimi.ai/docs/guide/kimi-k3-quickstart):
// reasoning_effort supports only "max", and the API rejects OpenAI-isms
// pi sends by default (store: false, developer role), hence the compat map.
export default function (pi) {
  pi.registerProvider("moonshot", {
    name: "Moonshot AI",
    baseUrl: "https://api.moonshot.ai/v1",
    apiKey: "$MOONSHOT_API_KEY",
    api: "openai-completions",
    models: [
      {
        id: "kimi-k3",
        name: "Kimi K3",
        reasoning: true,
        // Every pi thinking level maps to "max": it is the only effort K3 accepts.
        thinkingLevelMap: {
          off: "max",
          minimal: "max",
          low: "max",
          medium: "max",
          high: "max",
          xhigh: "max",
          max: "max",
        },
        input: ["text", "image"],
        // USD per MTok, platform.kimi.ai launch pricing (2026-07-16).
        cost: { input: 3.0, output: 15.0, cacheRead: 0.3, cacheWrite: 0 },
        contextWindow: 1048576,
        maxTokens: 131072,
        compat: { supportsStore: false, supportsDeveloperRole: false },
      },
    ],
  });
}
