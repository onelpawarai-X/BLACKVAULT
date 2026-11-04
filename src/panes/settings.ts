// BLACKVAULT — Settings. Every preference here is persisted and takes effect
// immediately; the core re-reads the settings row on each change.

import * as bridge from "../lib/bridge";
import { el, mount, switchControl } from "../lib/dom";

import { getState, setState } from "../lib/store";
import { paneHead, type Pane, type PaneCtx } from "../ui/pane";
import type { Settings } from "../lib/types";

export const settingsPane: Pane = {
  id: "settings",
  title: "Settings",
  render(host, ctx) {
    const s = getState().settings;
    if (!s) {
      mount(host, el("div", { class: "bv-empty" }, "Settings not loaded."));
      return;
    }

    const save = async (patch: Partial<Settings>): Promise<void> => {
      try {
        const next = await bridge.saveSettings({ ...getState().settings!, ...patch });
        setState({ settings: next });
      } catch (e) {
        ctx.toast("Could not save setting", String(e), "critical");
      }
    };

    mount(
      host,
      paneHead("Settings", "Preferences · persistence · startup"),
      group("Startup & background", [
        row("Start with Windows", "Register a startup entry so BLACKVAULT is always running.", s.startWithWindows, (v) => save({ startWithWindows: v })),
        row("Start minimised", "Launch straight to the tray.", s.startMinimized, (v) => save({ startMinimized: v })),
        row("Keep running when closed", "Closing the window keeps the voice engine alive in the tray.", s.minimizeToTray, (v) => save({ minimizeToTray: v })),
      ]),
      group("Identity", [
        textRow("Your name", s.ownerName, (v) => save({ ownerName: v })),
      ]),
      group("Voice engine", [
        row("Always-on listening", "Keep the microphone armed while unlocked.", s.alwaysOnVoice, (v) => save({ alwaysOnVoice: v })),
        row("Wake phrase", "Require the wake phrase before commands are accepted.", s.wakePhraseEnabled, (v) => save({ wakePhraseEnabled: v })),
        textRow("Wake phrase text", s.wakePhrase, (v) => save({ wakePhrase: v })),
        row("Require wake phrase for every command", "Turn off to let commands run without a wake word.", s.requireWakeForCommands, (v) => save({ requireWakeForCommands: v })),
        row("Listen while locked", "Keep recognising speech on the lock screen.", s.listenWhileLocked, (v) => save({ listenWhileLocked: v })),
        row("Spoken feedback", "Confirm commands with a short spoken acknowledgement.", s.voiceFeedback, (v) => save({ voiceFeedback: v })),
      ]),
      group("Speech recognition", [
        row(
          "Use the ZYRAXON speech-to-text service",
          "Recognises what you say through your own endpoint. Works even when Windows has no offline recogniser.",
          s.sttEnabled,
          (v) => save({ sttEnabled: v }),
        ),
        selectRow(
          "Language",
          s.sttLanguage,
          [
            ["en", "English"],
            ["bn", "Bengali"],
            ["both", "Both (English + Bengali)"],
          ],
          (v) => save({ sttLanguage: v }),
        ),
        row(
          "Live streaming",
          "Send audio while you are still speaking, so a command lands sooner.",
          s.sttLive,
          (v) => save({ sttLive: v }),
        ),
        textRow("Endpoint", s.sttEndpoint, (v) => save({ sttEndpoint: v })),
        numberRow(
          "Minimum confidence (0–100)",
          Math.round(s.sttMinConfidence * 100),
          (v) => save({ sttMinConfidence: Math.min(1, Math.max(0, v / 100)) }),
        ),
        sttTestRow(ctx),
      ]),
      group("Lock screen", [
        selectRow("Lock animation", s.lockAnimation, [["iris", "Iris collapse"], ["shutter", "Shutter"], ["dissolve", "Dissolve"]], (v) => save({ lockAnimation: v as Settings["lockAnimation"] })),
        selectRow("Unlock animation", s.unlockAnimation, [["bloom", "Light bloom"], ["rings", "Ripple rings"], ["vortex", "Vortex"]], (v) => save({ unlockAnimation: v as Settings["unlockAnimation"] })),
        noteRow("Mouse cursor", "Always visible on the lock screen, so the PIN field and pattern grid can be used."),
        row("Cover every monitor", "Deploy a black overlay on all displays, not just the primary.", s.multiMonitorOverlay, (v) => save({ multiMonitorOverlay: v })),
        numberRow("Auto-relock after (seconds, 0 = off)", s.autoRelockSecs, (v) => save({ autoRelockSecs: v })),
        row("Lock on sleep", "Lock when the machine sleeps.", s.lockOnSleep, (v) => save({ lockOnSleep: v })),
        row("Lock on screen saver", "Lock when the screen saver engages.", s.lockOnScreenSaver, (v) => save({ lockOnScreenSaver: v })),
        numberRow("Lock after idle (seconds, 0 = off)", s.lockOnIdleSecs, (v) => save({ lockOnIdleSecs: v })),
        row("Block task switching while locked", "Suppress Alt+Tab and the Windows key over the overlay.", s.blockTaskSwitchWhenLocked, (v) => save({ blockTaskSwitchWhenLocked: v })),
      ]),
      group("Authentication", [
        row("Require a second factor", "Always ask for a local factor after the voice phrase.", s.requireSecondFactor, (v) => save({ requireSecondFactor: v })),
        selectRow("Second factor", s.secondFactor, [["pin", "PIN"], ["pattern", "Pattern"], ["hello", "Windows Hello"], ["voiceprint", "Voiceprint"], ["passphrase", "Passphrase"]], (v) => save({ secondFactor: v as Settings["secondFactor"] })),
        numberRow("Voiceprint threshold (0–100)", Math.round(s.voiceprintThreshold * 100), (v) => save({ voiceprintThreshold: Math.min(1, Math.max(0, v / 100)) })),
        numberRow("Max failed attempts before cool-down", s.authRateLimitAttempts, (v) => save({ authRateLimitAttempts: v })),
        numberRow("Cool-down window (seconds)", s.authRateLimitWindowSecs, (v) => save({ authRateLimitWindowSecs: v })),
      ]),
      group("Privacy", [
        row("Store voice recordings", "Off by default. BLACKVAULT processes audio in memory and discards it.", s.storeAudio, (v) => save({ storeAudio: v })),
        row("Telemetry", "BLACKVAULT sends nothing. This cannot be turned on.", s.telemetry, () => undefined, true),
        numberRow("Audit log retention (days)", s.auditLogRetentionDays, (v) => save({ auditLogRetentionDays: v })),
      ]),
      group("Maintenance", [
        maintenanceRow(ctx),
      ]),
    );
  },
};

function group(title: string, rows: HTMLElement[]): HTMLElement {
  return el(
    "div",
    { class: "bv-section" },
    el("div", { class: "bv-section-title" }, title),
    el("div", { class: "bv-list", style: "padding:4px 0" }, ...rows),
  );
}

function row(label: string, note: string, value: boolean, onChange: (v: boolean) => void, locked = false): HTMLElement {
  const sw = switchControl(value, (v) => onChange(v));
  if (locked) sw.style.opacity = "0.5";
  return el(
    "div",
    { class: "bv-row" },
    el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, label), el("div", { class: "bv-row-meta" }, note)),
    sw,
  );
}

function noteRow(label: string, note: string): HTMLElement {
  return el(
    "div",
    { class: "bv-row" },
    el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, label), el("div", { class: "bv-row-meta" }, note)),
  );
}

function textRow(label: string, value: string, onChange: (v: string) => void): HTMLElement {
  const input = el("input", { class: "bv-input", value, style: "max-width:280px" });
  input.addEventListener("change", () => onChange(input.value.trim()));
  return el(
    "div",
    { class: "bv-row" },
    el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, label)),
    input,
  );
}

function numberRow(label: string, value: number, onChange: (v: number) => void): HTMLElement {
  const input = el("input", { class: "bv-input", type: "number", value, style: "max-width:140px" });
  input.addEventListener("change", () => onChange(Number(input.value) || 0));
  return el(
    "div",
    { class: "bv-row" },
    el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, label)),
    input,
  );
}

function selectRow(label: string, value: string, options: [string, string][], onChange: (v: string) => void): HTMLElement {
  const sel = el("select", { class: "bv-select", style: "max-width:220px" });
  for (const [v, l] of options) {
    const o = el("option", { value: v }, l);
    if (v === value) o.selected = true;
    sel.appendChild(o);
  }
  sel.addEventListener("change", () => onChange(sel.value));
  return el(
    "div",
    { class: "bv-row" },
    el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, label)),
    sel,
  );
}

function sttTestRow(ctx: PaneCtx): HTMLElement {
  const out = el("div", { class: "bv-row-meta", style: "min-height:18px" }, "Not tested yet.");
  const actions = el("div", { style: "display:flex;gap:8px;flex-wrap:wrap" }, out);
  const test = btn("Test connection", "ghost", async () => {
    out.textContent = "Contacting the endpoint\u2026";
    const current = getState().settings;
    if (!current) return;
    try {
      const t = await bridge.probeStt(current);
      out.textContent = t.text
        ? `Reachable \u00b7 ${t.locale || t.engine || "ok"} \u00b7 heard \u201c${t.text}\u201d`
        : `Reachable \u00b7 ${t.locale || t.engine || "ok"} \u00b7 silence recognised as empty text`;
    } catch (e) {
      out.textContent = `Failed: ${String(e)}`;
      ctx.toast("Speech service unreachable", String(e), "critical");
    }
  });
  actions.insertBefore(test, out);
  return el(
    "div",
    { class: "bv-row" },
    el("div", { class: "bv-row-main" },
      el("div", { class: "bv-row-name" }, "Connection"),
      el("div", { class: "bv-row-meta" }, "Sends half a second of silence and reports what comes back."),
    ),
    actions,
  );
}

function maintenanceRow(ctx: PaneCtx): HTMLElement {
  const actions = el("div", { style: "display:flex;gap:8px;flex-wrap:wrap" });
  actions.appendChild(btn("Open log folder", "ghost", async () => {
    try { await bridge.openLogFolder(); } catch (e) { ctx.toast("Failed", String(e), "critical"); }
  }));
  actions.appendChild(btn("Restart voice engine", "ghost", async () => {
    try {
      const v = await bridge.restartVoice();
      setState({ voice: v });
      ctx.toast("Voice engine restarted");
    } catch (e) { ctx.toast("Failed", String(e), "critical"); }
  }));
  actions.appendChild(btn("Export audit log", "ghost", async () => {
    try { const p = await bridge.exportAudit(); ctx.toast("Exported", p); } catch (e) { ctx.toast("Failed", String(e), "critical"); }
  }));
  actions.appendChild(btn("Factory reset", "danger", async () => {
    if (!confirm("This erases the BLACKVAULT database, all commands, protection rules and factors. Continue?")) return;
    try {
      await bridge.factoryReset();
      ctx.toast("Factory reset complete", "Restart BLACKVAULT.");
    } catch (e) { ctx.toast("Failed", String(e), "critical"); }
  }));

  return el(
    "div",
    { class: "bv-row" },
    el("div", { class: "bv-row-main" },
      el("div", { class: "bv-row-name" }, "Maintenance"),
      el("div", { class: "bv-row-meta" }, "Logs, engine recovery, audit export and a full reset."),
    ),
    actions,
  );
}

function btn(text: string, variant: string, onClick: () => void | Promise<void>): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant && variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(text));
  b.addEventListener("click", () => void onClick());
  return b;
}
