const SAFE_MCP_ENV_VARS = [
  "HOME",
  "LOGNAME",
  "PATH",
  "SHELL",
  "TERM",
  "TMP",
  "TMPDIR",
  "TEMP",
  "USER",
  "XDG_RUNTIME_DIR",
  "SSL_CERT_FILE",
  "NIX_SSL_CERT_FILE",
  "REQUESTS_CA_BUNDLE",
  "CURL_CA_BUNDLE",
  "GIT_CONFIG_NOSYSTEM",
  "GIT_SSL_CAINFO",
  "IX_GCAL_BIN",
  "IX_MCP_DASHBOARD_HTML",
  "IX_MCP_HOST",
  "IX_MCP_KERNEL_TRACE",
  "IX_MCP_PUBLIC_HOST",
  "IX_MCP_STORE",
  "IX_VMKIT_BIN",
];

const MODEL_PROVIDER_ENV_VARS = [
  "AI_GATEWAY_API_KEY",
  "ANT_LING_API_KEY",
  "ANTHROPIC_API_KEY",
  "ANTHROPIC_OAUTH_TOKEN",
  "AWS_ACCESS_KEY_ID",
  "AWS_BEARER_TOKEN_BEDROCK",
  "AWS_SECRET_ACCESS_KEY",
  "AWS_SECURITY_TOKEN",
  "AWS_SESSION_TOKEN",
  "AZURE_OPENAI_API_KEY",
  "CEREBRAS_API_KEY",
  "CLAUDE_API_KEY",
  "CLOUDFLARE_API_KEY",
  "CODEX_API_KEY",
  "COHERE_API_KEY",
  "DEEPSEEK_API_KEY",
  "FIREWORKS_API_KEY",
  "GEMINI_API_KEY",
  "GOOGLE_API_KEY",
  "GROQ_API_KEY",
  "KIMI_API_KEY",
  "MINIMAX_API_KEY",
  "MISTRAL_API_KEY",
  "MOONSHOT_API_KEY",
  "NVIDIA_API_KEY",
  "OPENCODE_API_KEY",
  "OPENAI_API_KEY",
  "OPENROUTER_API_KEY",
  "PERPLEXITY_API_KEY",
  "TOGETHER_API_KEY",
  "XIAOMI_API_KEY",
  "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
  "XIAOMI_TOKEN_PLAN_CN_API_KEY",
  "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
  "XAI_API_KEY",
  "ZAI_API_KEY",
  "ZAI_CODING_CN_API_KEY",
];

const EXTRA_ALLOWLIST_ENV = "PI_HARNESS_MCP_ENV_ALLOWLIST";
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

function parseExtraAllowlist(value) {
  if (typeof value !== "string" || value.trim() === "") return [];
  return value
    .split(",")
    .map((name) => name.trim())
    .filter((name) => ENV_NAME.test(name));
}

export function buildMcpEnv(source = process.env) {
  const blocked = new Set(MODEL_PROVIDER_ENV_VARS);
  const names = new Set([
    ...SAFE_MCP_ENV_VARS,
    ...parseExtraAllowlist(source[EXTRA_ALLOWLIST_ENV]),
  ]);
  const env = {};

  for (const name of names) {
    if (blocked.has(name)) continue;
    const value = source[name];
    if (typeof value === "string") env[name] = value;
  }

  return env;
}

export { EXTRA_ALLOWLIST_ENV, MODEL_PROVIDER_ENV_VARS, SAFE_MCP_ENV_VARS };
