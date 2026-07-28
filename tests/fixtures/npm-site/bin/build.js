import { mkdir, writeFile } from "node:fs/promises";
import { clsx } from "clsx";

const classFlag = process.argv.indexOf("--class");
const className = classFlag === -1 ? "ix npm missing-flag" : process.argv[classFlag + 1];

await mkdir("dist", { recursive: true });
await writeFile("dist/index.html", `<main class="${clsx(className)}">npm lock OK</main>\n`);
