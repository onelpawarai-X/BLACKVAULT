// BLACKVAULT — Automation. Every keyboard, mouse, media and volume action is a
// real SendInput call in the Rust core. This pane is the manual test surface.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { paneHead, type Pane, type PaneCtx } from "../ui/pane";

const KEY_COMBOS: [string, string][] = [
  ["ctrl+c", "Copy"],
  ["ctrl+v", "Paste"],
  ["ctrl+z", "Undo"],
  ["alt+tab", "Switch window"],
  ["win+d", "Show desktop"],
  ["ctrl+shift+esc", "Task manager"],
  ["alt+f4", "Close window"],
  ["f5", "Refresh"],
  ["enter", "Enter"],
  ["space", "Space"],
  ["esc", "Escape"],
  ["left", "Left arrow"],
  ["right", "Right arrow"],
  ["up", "Up arrow"],
  ["down", "Down arrow"],
];

export const automationPane: Pane = {
  id: "automation",
  title: "Automation",
  render(host, ctx) {
    mount(
      host,
      paneHead("Automation", "Drive the desktop by voice — keys, mouse, media, volume"),
      section("Media & volume", [
        tile("Next", ICONS.next, () => run(ctx, "media_next")),
        tile("Previous", ICONS.prev, () => run(ctx, "media_prev")),
        tile("Play / Pause", ICONS.play, () => run(ctx, "media_play_pause")),
        tile("Volume Up", ICONS.volumeUp, () => run(ctx, "volume_up")),
        tile("Volume Down", ICONS.volumeDown, () => run(ctx, "volume_down")),
        tile("Mute", ICONS.volumeMute, () => run(ctx, "volume_mute")),
      ]),
      section("Mouse", [
        tile("Click here", ICONS.mouse, () => run(ctx, "mouse_click")),
        tile("Scroll Down", ICONS.scroll, () => run(ctx, "scroll", "down:600")),
        tile("Scroll Up", ICONS.scroll, () => run(ctx, "scroll", "up:600")),
      ]),
      section("Windows", [
        tile("Minimise all", ICONS.windows, () => run(ctx, "minimize_all")),
        tile("Show desktop", ICONS.monitor, () => run(ctx, "show_desktop")),
        tile("Screenshot", ICONS.camera, () => run(ctx, "screenshot")),
      ]),
      section("Key combos", [
        ...KEY_COMBOS.map(([combo, label]) =>
          tile(label, ICONS.keyboard, () => sendKeys(ctx, combo), combo),
        ),
      ]),
    );
  },
};

function section(title: string, tiles: HTMLElement[]): HTMLElement {
  return el(
    "div",
    { class: "bv-section" },
    el("div", { class: "bv-section-title" }, title),
    el("div", { class: "bv-grid-cards" }, ...tiles),
  );
}

function tile(label: string, icon: string, onClick: () => void, sub?: string): HTMLElement {
  const card = el(
    "div",
    { class: "bv-card", style: "cursor:pointer;text-align:left", role: "button", tabindex: 0 },
    el("div", { class: "bv-card-title" }, svgIcon(icon, 16), label),
    sub ? el("div", { class: "bv-metric-note" }, sub) : el("div", { class: "bv-metric-note" }, "Tap to send"),
  );
  card.addEventListener("click", onClick);
  card.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onClick(); }
  });
  return card;
}

async function run(ctx: PaneCtx, action: string, payload?: string): Promise<void> {
  try {
    await bridge.runAutomation(action, payload ?? null);
    ctx.toast("Sent", action);
  } catch (e) {
    ctx.toast("Action failed", String(e), "critical");
  }
}

async function sendKeys(ctx: PaneCtx, combo: string): Promise<void> {
  try {
    await bridge.pressKeys(combo);
    ctx.toast("Keys sent", combo);
  } catch (e) {
    ctx.toast("Key send failed", String(e), "critical");
  }
}
