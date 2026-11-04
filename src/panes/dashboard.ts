// BLACKVAULT — Dashboard: the command centre. Everything here is live state.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, relTime } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { getState, navigate } from "../lib/store";
import { paneHead, type Pane } from "../ui/pane";

export const dashboardPane: Pane = {
  id: "dashboard",
  title: "Dashboard",
  render(host, ctx) {
    const st = getState();
    const status = st.status;

    const lockBtn = el(
      "button",
      { class: "bv-btn primary", type: "button" },
      svgIcon(ICONS.lock, 15),
    );
    lockBtn.appendChild(document.createTextNode("Lock Now"));
    lockBtn.addEventListener("click", async () => {
      try {
        await bridge.lockPc("manual");
      } catch (e) {
        ctx.toast("Lock failed", String(e), "critical");
      }
    });

    const mode = status?.mode ?? "unlocked";
    mount(
      host,
      paneHead("Command Centre", "Live security posture", lockBtn),
      el(
        "div",
        { class: "bv-grid-cards" },
        metric("Security Mode", modeLabel(mode), modeNote(mode), ICONS.shieldCheck, mode),
        metric(
          "Voice Engine",
          st.voice?.running ? "Active" : "Stopped",
          st.voice?.lastCommand ? `Last: ${st.voice.lastCommand}` : st.voice?.backend ? `Engine: ${st.voice.backend}` : "—",
          ICONS.mic,
          st.voice?.running ? "ok" : "off",
        ),
        metric(
          "Speech Recogniser",
          status?.speechAvailable ? "Ready" : "Unavailable",
          status?.speechAvailable
            ? "Offline · Windows SAPI"
            : "No offline recogniser installed — commands need a taught phrase",
          ICONS.activity,
          status?.speechAvailable ? "ok" : "warn",
        ),
        metric(
          "Microphone",
          st.mic?.muted ? "Muted" : st.mic?.permission === "granted" ? "Live" : "Unknown",
          st.mic?.deviceName ?? "No device",
          ICONS.activity,
          st.mic?.permission === "granted" && !st.mic?.muted ? "ok" : "warn",
        ),
        metric(
          "Windows Hello",
          st.hello?.available ? (st.hello.enrolled ? "Enrolled" : "Available") : "Unavailable",
          st.hello?.method ?? "Not configured",
          ICONS.fingerprint,
          st.hello?.enrolled ? "ok" : "warn",
        ),
        metric(
          "PIN",
          status?.pinConfigured ? "Set" : "Not set",
          status?.pinConfigured ? "Local hash only" : "Add one in Security",
          ICONS.pin,
          status?.pinConfigured ? "ok" : "warn",
        ),
        metric(
          "Pattern",
          status?.patternConfigured ? "Set" : "Not set",
          status?.patternConfigured ? "3×3 grid enrolled" : "Add one in Security",
          ICONS.grid,
          status?.patternConfigured ? "ok" : "warn",
        ),
        metric("Protected Apps", String(status?.protectedApps ?? 0), "App Lock rules", ICONS.app, "ok"),
        metric(
          "Protected Files",
          String((status?.protectedFiles ?? 0) + (status?.protectedFolders ?? 0)),
          `${status?.protectedFiles ?? 0} files · ${status?.protectedFolders ?? 0} folders`,
          ICONS.folder,
          "ok",
        ),
        metric(
          "Background Service",
          status?.serviceRunning ? "Running" : "Stopped",
          `Uptime ${fmtUptime(status?.uptimeSecs ?? 0)}`,
          ICONS.cpu,
          status?.serviceRunning ? "ok" : "off",
        ),
      ),
      el(
        "div",
        { class: "bv-split", style: "margin-top:16px" },
        eventsCard(st.events.slice(0, 6), () => navigate("events")),
        authCard(status?.lastAuth ?? null, st.hello),
      ),
    );
  },
};

function modeLabel(mode: string): string {
  return { unlocked: "Unlocked", locked: "Locked", locking: "Locking", unlocking: "Unlocking" }[mode] ?? mode;
}
function modeNote(mode: string): string {
  return {
    unlocked: "Desktop visible · voice armed",
    locked: "Full-screen privacy mode active",
    locking: "Overlay deploying",
    unlocking: "Verifying identity",
  }[mode] ?? "";
}

function metric(
  title: string,
  value: string,
  note: string,
  icon: string,
  tone: string,
): HTMLElement {
  return el(
    "div",
    { class: "bv-card" },
    el("div", { class: "bv-card-title" }, svgIcon(icon, 16), title),
    el("div", { class: "bv-metric", style: "font-size:24px" }, value),
    el(
      "div",
      { class: "bv-metric-note" },
      el("span", { class: `bv-dot ${tone === "ok" ? "on" : tone === "off" ? "off" : "warn"}`, style: "display:inline-block;margin-right:7px" }),
      note,
    ),
  );
}

function eventsCard(events: { id: number; kind: string; message: string; at: string; severity: string }[], more: () => void): HTMLElement {
  const rows = events.length
    ? events.map((e) =>
        el(
          "div",
          { class: "bv-row" },
          el("span", { class: `bv-dot ${e.severity === "critical" ? "off" : e.severity === "warn" ? "warn" : "on"}` }),
          el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, e.message), el("div", { class: "bv-row-meta" }, e.kind)),
          el("div", { class: "bv-row-meta" }, relTime(e.at)),
        ),
      )
    : [el("div", { class: "bv-empty" }, "No security events recorded yet")];

  const btn = el("button", { class: "bv-btn ghost sm", type: "button" });
  btn.appendChild(document.createTextNode("View all"));
  btn.addEventListener("click", more);

  return el(
    "div",
    { class: "bv-card" },
    el("div", { class: "bv-card-title" }, svgIcon(ICONS.history, 16), "Recent Security Events"),
    el("div", { class: "bv-list" }, ...rows),
    el("div", { style: "margin-top:12px;display:flex;justify-content:flex-end" }, btn),
  );
}

function authCard(lastAuth: string | null, hello: { available: boolean; enrolled: boolean } | null): HTMLElement {
  return el(
    "div",
    { class: "bv-card" },
    el("div", { class: "bv-card-title" }, svgIcon(ICONS.user, 16), "Authentication"),
    el(
      "div",
      {},
      kv("Last authentication", relTime(lastAuth)),
      kv("Windows Hello", hello?.available ? (hello.enrolled ? "Enrolled" : "Available, not enrolled") : "Unavailable"),
      kv("Voice identity", getState().status?.passphraseConfigured ? "Phrase configured" : "Not configured"),
      kv("Rate limiting", "Active"),
      kv("Audit retention", `${getState().settings?.auditLogRetentionDays ?? 90} days`),
    ),
    el(
      "div",
      { class: "bv-hint", style: "margin-top:14px" },
      "Voice alone is never sufficient for protected resources. A local factor (PIN, pattern or Windows Hello) is always required.",
    ),
  );
}

function kv(k: string, v: string): HTMLElement {
  return el("div", { class: "bv-kv" }, el("span", { class: "bv-kv-k" }, k), el("span", { class: "bv-kv-v" }, v));
}

export function fmtUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}
