import { spawn } from "node:child_process";

// Run a fully isolated, headless `pi` turn and return its stdout/stderr.
//
// The child sees ONLY the prompt we pass - no parent session, no parent
// transcript, no parent reasoning (--no-session, fresh process). That isolation
// is the entire mechanism behind the prosecutor: with no shared context the two
// agents cannot launder each other's hallucinations. The child keeps its
// built-in tools (no --no-builtin-tools) so it can actually probe: run tests,
// git diff, grep.
export function runIsolatedPi({
  prompt,
  systemPrompt,
  provider,
  model,
  thinking,
  cwd,
  timeoutMs = 120000,
  signal,
}) {
  return new Promise((resolve, reject) => {
    const args = ["--print", "--no-session"];
    if (provider) args.push("--provider", provider);
    if (model) args.push("--model", model);
    if (thinking) args.push("--thinking", thinking);
    if (systemPrompt) args.push("--system-prompt", systemPrompt);
    args.push(prompt);

    const child = spawn("pi", args, { cwd, signal, env: process.env });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);

    child.stdout.on("data", (d) => {
      stdout += d;
    });
    child.stderr.on("data", (d) => {
      stderr += d;
    });
    child.on("error", (err) => {
      clearTimeout(timer);
      reject(err);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}
