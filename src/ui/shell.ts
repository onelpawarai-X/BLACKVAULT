// BLACKVAULT — application shell: top glass nav, pane router, toast host.

import { el, mount, svgIcon, toast as domToast } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { getState, navigate, subscribe } from "../lib/store";
import { relTime } from "../lib/dom";
import type { Pane, PaneCtx } from "./pane";
import { dashboardPane } from "../panes/dashboard";
import { agentPane } from "../panes/agent";
import { voicePane } from "../panes/voice";
import { launcherPane } from "../panes/launcher";
import { appsPane } from "../panes/apps";
import { filesPane } from "../panes/files";
import { securityPane } from "../panes/security";
import { automationPane } from "../panes/automation";
import { eventsPane } from "../panes/events";
import { settingsPane } from "../panes/settings";

interface RouteDef {
  id: string;
  label: string;
  icon: string;
  pane: Pane;
}

const ROUTES: RouteDef[] = [
  { id: "dashboard", label: "Dashboard", icon: ICONS.vault, pane: dashboardPane },
  { id: "agent", label: "Agent", icon: ICONS.agent, pane: agentPane },
  { id: "voice", label: "Voice", icon: ICONS.mic, pane: voicePane },
  { id: "launcher", label: "Launcher", icon: ICONS.rocket, pane: launcherPane },
  { id: "apps", label: "App Lock", icon: ICONS.app, pane: appsPane },
  { id: "files", label: "Files", icon: ICONS.folder, pane: filesPane },
  { id: "security", label: "Security", icon: ICONS.fingerprint, pane: securityPane },
  { id: "automation", label: "Automation", icon: ICONS.zap, pane: automationPane },
  { id: "events", label: "Events", icon: ICONS.history, pane: eventsPane },
  { id: "settings", label: "Settings", icon: ICONS.settings, pane: settingsPane },
];

export function createShell(host: HTMLElement): { toastHost: HTMLElement } {
  const tabsEl = el("div", { class: "bv-nav-tabs" });
  const navRight = el("div", { class: "bv-nav-right" });
  const main = el("div", { class: "bv-main" });
  const toastHost = el("div", { class: "bv-toasts" });

  const nav = el(
    "header",
    { class: "bv-nav" },
    el(
      "div",
      { class: "bv-brand" },
      brandMark(),
      el(
        "div",
        {},
        el("div", { class: "bv-brand-name" }, "BLACKVAULT"),
        el("div", { class: "bv-brand-tag" }, "YOUR VOICE · YOUR CONTROL"),
      ),
    ),
    tabsEl,
    navRight,
  );

  const shell = el(
    "div",
    { class: "bv-shell" },
    nav,
    main,
  );

  mount(host, el("div", { class: "bv-ambient" }), el("div", { class: "bv-grid" }), shell, toastHost);

  const ctx: PaneCtx = {
    toast: (title, body, severity) => domToast(toastHost, title, body, severity),
    host: main,
  };

  const tabButtons = new Map<string, HTMLButtonElement>();
  for (const r of ROUTES) {
    const b = el("button", { class: "bv-tab", type: "button" }) as HTMLButtonElement;
    b.appendChild(svgIcon(r.icon, 15));
    b.appendChild(document.createTextNode(r.label));
    b.addEventListener("click", () => navigate(r.id));
    tabsEl.appendChild(b);
    tabButtons.set(r.id, b);
  }

  // One pane element per route, kept alive across navigation. Panes keep their
  // own state and their own listeners; the shell never disposes them, so there
  // is no teardown to get wrong when the user moves between tabs.
  const paneCache = new Map<string, HTMLElement>();
  let activeId = "";

  function render(): void {
    const st = getState();

    for (const [id, b] of tabButtons) b.classList.toggle("is-active", id === st.route);

    // Status cluster on the right of the nav. The microphone is reported by
    // permission and mute state only; the live input level is pushed straight
    // at the lock-screen waveform and never enters the store, so this row does
    // not flicker as the level moves.
    mount(
      navRight,
      statusPill(st.status?.mode ?? "unlocked"),
      el(
        "div",
        { class: "bv-status", title: st.voice?.running ? "Voice engine running" : "Voice engine stopped" },
        el("span", { class: `bv-dot ${st.voice?.running ? "on" : "off"}` }),
        st.voice?.running ? "Voice" : "Voice Off",
      ),
      el(
        "div",
        { class: "bv-status", title: st.mic?.deviceName || "Microphone" },
        el("span", { class: `bv-dot ${st.mic?.muted ? "off" : st.mic?.permission === "granted" ? "on" : "warn"}` }),
        st.mic?.muted ? "Mic Muted" : st.mic?.permission === "granted" ? "Mic Live" : "Mic ?",
      ),
      st.version
        ? el("div", { class: "bv-version", title: `BLACKVAULT ${st.version}` }, `v${st.version}`)
        : null,
    );

    const def = ROUTES.find((r) => r.id === st.route) ?? ROUTES[0]!;

    let node = paneCache.get(def.id);
    const first = !node;
    if (!node) {
      node = el("section", { class: "bv-pane" });
      paneCache.set(def.id, node);
    }

    if (first || activeId !== def.id) {
      mount(main, node);
      activeId = def.id;
      // A pane that looks after its own updates keeps the paint it already has.
      if (first || !def.pane.selfUpdating) def.pane.render(node, ctx);
      return;
    }

    // Same pane, new state. Two panes are left alone: one that maintains itself
    // from its own event stream (rebuilding it would drop the chat and its
    // listener), and one the user is typing into (rebuilding it would take the
    // caret). Everything else repaints, which is how live values reach a pane
    // that was painted before the core had answered.
    if (def.pane.selfUpdating) return;
    if (node.contains(document.activeElement)) return;
    def.pane.render(node, ctx);
  }

  subscribe(render);
  render();

  return { toastHost };
}

function statusPill(mode: string): HTMLElement {
  const map: Record<string, string> = {
    locked: "off",
    locking: "warn",
    unlocking: "warn",
    unlocked: "on",
  };
  const label: Record<string, string> = {
    locked: "Locked",
    locking: "Locking",
    unlocking: "Unlocking",
    unlocked: "Unlocked",
  };
  return el(
    "div",
    { class: "bv-status", title: "Security mode" },
    el("span", { class: `bv-dot ${map[mode] ?? "warn"}` }),
    label[mode] ?? mode,
  );
}

function brandMark(): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", "bv-brand-mark");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.4");
  const outer = document.createElementNS("http://www.w3.org/2000/svg", "path");
  outer.setAttribute("d", ICONS.vault!);
  const inner = document.createElementNS("http://www.w3.org/2000/svg", "circle");
  inner.setAttribute("cx", "12");
  inner.setAttribute("cy", "11.4");
  inner.setAttribute("r", "2.6");
  svg.appendChild(outer);
  svg.appendChild(inner);
  return svg;
}

/** Splash shown while the core boots. */
export function splash(host: HTMLElement): () => void {
  const node = el(
    "div",
    { class: "bv-splash" },
    (() => {
      const svg = brandMark();
      svg.setAttribute("class", "bv-splash-mark");
      svg.setAttribute("stroke-width", "1");
      return svg;
    })(),
    el("div", { class: "bv-splash-name" }, "BLACKVAULT"),
    el("div", { class: "bv-splash-tag" }, "YOUR VOICE · YOUR CONTROL · YOUR SECURITY"),
    el("div", { class: "bv-splash-bar" }, el("i", {})),
  );
  host.appendChild(node);
  return () => {
    node.classList.add("leaving");
    setTimeout(() => node.remove(), 720);
  };
}

export { relTime };
