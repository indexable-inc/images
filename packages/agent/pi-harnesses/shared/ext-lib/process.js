import { spawn } from "node:child_process";

export function runProcess(cmd, args, opts = {}) {
  return new Promise((resolve) => {
    const child = spawn(cmd, args, opts);
    let stdout = "";
    let stderr = "";
    child.stdout?.on("data", (data) => {
      stdout += data;
    });
    child.stderr?.on("data", (data) => {
      stderr += data;
    });
    child.on("error", (error) => resolve({ code: 127, stdout, stderr: String(error) }));
    child.on("close", (code) => resolve({ code: code ?? 0, stdout, stderr }));
  });
}

export function countDiffLines(numstat) {
  return numstat
    .trim()
    .split("\n")
    .filter(Boolean)
    .reduce((total, line) => {
      const [added, removed] = line.split("\t");
      const addedCount = Number.parseInt(added, 10);
      const removedCount = Number.parseInt(removed, 10);
      return total + (Number.isFinite(addedCount) ? addedCount : 0) + (Number.isFinite(removedCount) ? removedCount : 0);
    }, 0);
}
