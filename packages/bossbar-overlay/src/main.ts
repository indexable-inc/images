import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

interface BossBar {
  id: number;
  title: string;
  progress: number;
  color: string;
  overlay: string;
  position: number;
}

const COLORS = new Set([
  "pink",
  "blue",
  "red",
  "green",
  "yellow",
  "purple",
  "white",
]);
// Overlay values that draw notch sprites; "progress" is smooth (no overlay).
const NOTCHED = new Set([
  "notched_6",
  "notched_10",
  "notched_12",
  "notched_20",
]);

interface BarEls {
  block: HTMLDivElement;
  title: HTMLDivElement;
  fill: HTMLDivElement;
  notchFill: HTMLDivElement;
  color: string;
  overlay: string;
}

const root = document.getElementById("bars") as HTMLDivElement;
const bars = new Map<number, BarEls>();

function layer(...classes: string[]): HTMLDivElement {
  const el = document.createElement("div");
  el.className = ["bar__layer", ...classes].join(" ");
  return el;
}

function createBar(): BarEls {
  const block = document.createElement("div");
  block.className = "bar";

  const title = document.createElement("div");
  title.className = "bar__title";

  const track = document.createElement("div");
  track.className = "bar__track";

  const bg = layer("bar__bg");
  const fill = layer("bar__fill");
  const notchBg = layer("bar__notch-bg");
  const notchFill = layer("bar__notch-fill");

  // Order matters: it is the game's draw order, back to front.
  track.append(bg, fill, notchBg, notchFill);
  block.append(title, track);
  return { block, title, fill, notchFill, color: "", overlay: "" };
}

function updateBar(els: BarEls, b: BossBar): void {
  const color = COLORS.has(b.color) ? b.color : "purple";
  if (color !== els.color) {
    if (els.color) els.block.classList.remove(`c-${els.color}`);
    els.block.classList.add(`c-${color}`);
    els.color = color;
  }

  const overlay = NOTCHED.has(b.overlay) ? b.overlay : "";
  if (overlay !== els.overlay) {
    if (els.overlay) els.block.classList.remove(`o-${els.overlay}`);
    if (overlay) els.block.classList.add(`o-${overlay}`);
    els.overlay = overlay;
  }

  els.title.textContent = b.title;
  els.title.style.display = b.title ? "" : "none";

  const pct = `${Math.max(0, Math.min(1, b.progress)) * 100}%`;
  els.fill.style.width = pct;
  els.notchFill.style.width = pct;
}

function render(list: BossBar[]): void {
  const seen = new Set<number>();
  for (const b of list) {
    seen.add(b.id);
    let els = bars.get(b.id);
    if (!els) {
      els = createBar();
      bars.set(b.id, els);
    }
    updateBar(els, b);
    // appendChild moves an existing node, so the DOM ends up in payload order.
    root.appendChild(els.block);
  }
  for (const [id, els] of bars) {
    if (!seen.has(id)) {
      els.block.remove();
      bars.delete(id);
    }
  }
}

async function main(): Promise<void> {
  await listen<BossBar[]>("bossbars", (event) => render(event.payload));
  try {
    render(await invoke<BossBar[]>("get_bars"));
  } catch (e) {
    console.error("get_bars failed", e);
  }
}

void main();
