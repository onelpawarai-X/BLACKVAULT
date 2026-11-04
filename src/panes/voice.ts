// BLACKVAULT — Voice pane: engine control, wake phrase, live recognition,
// the command editor and the voice-identity (voiceprint) enrolment flow.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, relTime } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { getState, setState } from "../lib/store";
import { paneHead, type Pane, type PaneCtx } from "../ui/pane";
import type { VoiceCommand } from "../lib/types";

const ACTION_LABELS: Record<string, string> = {
  lock: "Lock the PC",
  unlock: "Unlock the PC",
  launch_app: "Launch an application",
  close_app: "Close an application",
  open_url: "Open a URL",
  key_press: "Press keys",
  mouse_click: "Click the mouse",
  scroll: "Scroll",
  media_next: "Media · next",
  media_prev: "Media · previous",
  media_play_pause: "Media · play / pause",
  volume_up: "Volume up",
  volume_down: "Volume down",
  volume_mute: "Mute",
  run_program: "Run a program",
  open_folder: "Open a folder",
  minimize_all: "Minimise everything",
  show_desktop: "Show desktop",
  screenshot: "Take a screenshot",
  custom_script: "Custom script",
};

export const voicePane: Pane = {
  id: "voice",
  title: "Voice",
  render(host, ctx) {
    const st = getState();
    const v = st.voice;

    const toggle = el("button", { class: "bv-btn primary", type: "button" });
    toggle.appendChild(svgIcon(v?.running ? ICONS.pause : ICONS.play, 15));
    toggle.appendChild(document.createTextNode(v?.running ? "Stop Engine" : "Start Engine"));
    toggle.addEventListener("click", async () => {
      try {
        const next = v?.running ? await bridge.stopVoice() : await bridge.startVoice();
        setState({ voice: next });
        ctx.toast(next.running ? "Voice engine started" : "Voice engine stopped");
      } catch (e) {
        ctx.toast("Voice engine error", String(e), "critical");
      }
    });

    const restart = el("button", { class: "bv-btn", type: "button" });
    restart.appendChild(svgIcon(ICONS.refresh, 15));
    restart.appendChild(document.createTextNode("Recover"));
    restart.addEventListener("click", async () => {
      try {
        const next = await bridge.restartVoice();
        setState({ voice: next });
        ctx.toast("Voice engine recovered");
      } catch (e) {
        ctx.toast("Recovery failed", String(e), "critical");
      }
    });

    // Live waveform + heard text
    const wave = el("div", { class: `bv-wave${v?.listening ? " is-live" : ""}` });
    for (let i = 0; i < 28; i++) {
      const b = el("span", {});
      b.style.animationDelay = `${(i * 53) % 900}ms`;
      b.style.height = v?.listening ? "" : "12%";
      wave.appendChild(b);
    }

    mount(
      host,
      paneHead("Voice Engine", "Offline recognition · always-on when armed", toggle, restart),
      el(
        "div",
        { class: "bv-grid-cards" },
        el(
          "div",
          { class: "bv-card", style: "grid-column: span 2" },
          el("div", { class: "bv-card-title" }, svgIcon(ICONS.wave, 16), "Live Recognition"),
          el(
            "div",
            { style: "display:flex;align-items:center;gap:18px" },
            el(
              "div",
              { style: "flex:1" },
              wave,
              el(
                "div",
                { class: "bv-row-meta", style: "margin-top:10px;font-size:12px" },
                st.heardFinal && st.heard ? `Heard: "${st.heard}"` : st.heard ? `…${st.heard}` : "Waiting for speech",
              ),
            ),
            el(
              "div",
              { style: "text-align:right" },
              el("div", { class: `bv-badge ${v?.running ? "ok" : "danger"}` }, v?.running ? "Armed" : "Disarmed"),
              el("div", { class: "bv-metric-note" }, `Backend: ${v?.backend ?? "none"}`),
              el("div", { class: "bv-metric-note" }, `Recoveries: ${v?.recoveryAttempts ?? 0}`),
            ),
          ),
        ),
        metricCard("Wake Phrase", st.settings?.wakePhraseEnabled ? `"${st.settings?.wakePhrase}"` : "Disabled", st.settings?.requireWakeForCommands ? "Required before commands" : "Commands run without wake", ICONS.bell),
        metricCard("Last Command", v?.lastCommand ?? "—", v?.lastHeard ? `Heard: ${v.lastHeard}` : "No command yet", ICONS.terminal),
        sttCard(ctx),
        identityCard(ctx),
      ),
      commandEditor(ctx),
    );
  },
};

function metricCard(title: string, value: string, note: string, icon: string): HTMLElement {
  return el(
    "div",
    { class: "bv-card" },
    el("div", { class: "bv-card-title" }, svgIcon(icon, 16), title),
    el("div", { class: "bv-metric-sm", style: "font-size:15px" }, value),
    el("div", { class: "bv-metric-note" }, note),
  );
}

/// The word-recognition card: this is the component that turns speech into
/// commands, so it gets its own status block rather than hiding inside Settings.
function sttCard(ctx: PaneCtx): HTMLElement {
  const st = getState();
  const stt = st.status?.stt;
  const running = !!stt?.running;
  const enabled = !!stt?.enabled;
  const note = stt?.lastError
    ? `Error: ${stt.lastError}`
    : stt?.lastText
      ? `Last heard: ${stt.lastText}`
      : enabled
        ? `Waiting \u00b7 ${stt?.recognised ?? 0} utterances processed`
        : "Disabled in Settings";

  const toggle = el("button", { class: "bv-btn", type: "button" });
  toggle.appendChild(document.createTextNode(enabled ? "Disable" : "Enable"));
  toggle.addEventListener("click", async () => {
    const current = getState().settings;
    if (!current) return;
    try {
      const next = await bridge.saveSettings({ ...current, sttEnabled: !enabled });
      setState({ settings: next });
      setState({ status: await bridge.getStatus() });
      ctx.toast(next.sttEnabled ? "Speech recognition enabled" : "Speech recognition disabled");
    } catch (e) {
      ctx.toast("Could not change the setting", String(e), "critical");
    }
  });

  return el(
    "div",
    { class: "bv-card" },
    el(
      "div",
      { class: "bv-card-title" },
      svgIcon(ICONS.activity, 16),
      "Speech Recognition",
      el("span", { class: `bv-badge ${stt?.lastError ? "warn" : running ? "ok" : "danger"}`, style: "margin-left:auto" },
        stt?.lastError ? "Fault" : running ? "Active" : enabled ? "Starting" : "Off"),
    ),
    el("div", { class: "bv-metric-note", style: "margin:8px 0 4px" }, note),
    el("div", { class: "bv-metric-note" }, `Endpoint: ${stt?.endpoint ?? "—"}`),
    el("div", { class: "bv-metric-note" }, `Language: ${stt?.language ?? "—"} \u00b7 ${stt?.live ? "streaming" : "whole utterance"}`),
    el("div", { style: "margin-top:10px;display:flex;gap:8px" }, toggle),
  );
}

function identityCard(ctx: PaneCtx): HTMLElement {
  const card = el("div", { class: "bv-card" });
  const body = el("div", {});
  card.appendChild(el("div", { class: "bv-card-title" }, svgIcon(ICONS.fingerprint, 16), "Voice Identity"));
  card.appendChild(body);

  const refresh = async () => {
    try {
      const id = await bridge.getVoiceIdentity();
      mount(
        body,
        el("div", { class: "bv-metric-sm", style: "font-size:15px" }, id.enrolled ? "Enrolled" : "Not enrolled"),
        el("div", { class: "bv-metric-note" }, `${id.sampleCount} sample(s) · threshold ${(id.threshold * 100).toFixed(0)}%`),
        el("div", { class: "bv-metric-note" }, id.updatedAt ? `Updated ${relTime(id.updatedAt)}` : "Capture 3 samples to enrol"),
        el(
          "div",
          { style: "display:flex;gap:8px;margin-top:12px" },
          btn("Enrol voice", "default", async () => {
            try {
              ctx.toast("Capturing voice samples", "Say your unlock phrase three times, clearly.");
              const samples = await captureSamples(3);
              const res = await bridge.enrollVoice(samples);
              ctx.toast("Voice identity enrolled", `${res.sampleCount} samples stored`);
              void refresh();
            } catch (e) {
              ctx.toast("Enrolment failed", String(e), "critical");
            }
          }),
          btn("Clear", "danger", async () => {
            try {
              await bridge.clearVoiceIdentity();
              ctx.toast("Voice identity cleared");
              void refresh();
            } catch (e) {
              ctx.toast("Clear failed", String(e), "critical");
            }
          }),
        ),
      );
    } catch (e) {
      mount(body, el("div", { class: "bv-empty" }, String(e)));
    }
  };
  void refresh();
  return card;
}

/** Record N short mic samples as Float32 → number[] for the core to average. */
async function captureSamples(n: number): Promise<number[]> {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const ctx = new AudioContext();
  const src = ctx.createMediaStreamSource(stream);
  const analyser = ctx.createAnalyser();
  analyser.fftSize = 2048;
  src.connect(analyser);
  const buf = new Float32Array(analyser.fftSize);
  const out: number[] = [];

  for (let i = 0; i < n; i++) {
    // ~900ms window per sample.
    const frames: number[][] = [];
    const t0 = performance.now();
    while (performance.now() - t0 < 900) {
      analyser.getFloatTimeDomainData(buf);
      frames.push(Array.from(buf));
      await new Promise((r) => setTimeout(r, 40));
    }
    // Flatten to a fixed-length vector (mean-pooled).
    const flat = frames.flat();
    const target = 512;
    const step = Math.max(1, Math.floor(flat.length / target));
    const vec: number[] = [];
    for (let k = 0; k < flat.length; k += step) vec.push(flat[k]!);
    out.push(...vec.slice(0, target));
    await new Promise((r) => setTimeout(r, 350));
  }
  stream.getTracks().forEach((t) => t.stop());
  await ctx.close();
  return out;
}

function commandEditor(ctx: PaneCtx): HTMLElement {
  const section = el("div", { class: "bv-section" });
  const listEl = el("div", { class: "bv-list" });

  const add = el("button", { class: "bv-btn primary sm", type: "button" });
  add.appendChild(svgIcon(ICONS.plus, 14));
  add.appendChild(document.createTextNode("New Command"));
  add.addEventListener("click", () => openCommandModal(null, ctx, refresh));

  section.appendChild(
    el(
      "div",
      { class: "bv-section-title" },
      "Voice Command Editor",
      el("div", { class: "bv-spacer" }),
      add,
    ),
  );
  section.appendChild(listEl);

  function refresh(): void {
    const cmds = getState().commands;
    if (!cmds.length) {
      mount(listEl, el("div", { class: "bv-empty" }, "No commands yet. Create one to control BLACKVAULT by voice."));
      return;
    }
    mount(
      listEl,
      ...cmds.map((c) =>
        el(
          "div",
          { class: "bv-row" },
          el("span", { class: `bv-dot ${c.enabled ? "on" : "off"}` }),
          el(
            "div",
            { class: "bv-row-main" },
            el("div", { class: "bv-row-name" }, `"${c.phrase}"`),
            el(
              "div",
              { class: "bv-row-meta" },
              `${ACTION_LABELS[c.actionType] ?? c.actionType}${c.payload ? ` → ${c.payload}` : ""}${c.requiresAuth ? " · requires auth" : ""}${c.builtin ? " · built-in" : ""} · used ${c.hitCount}×`,
            ),
          ),
          el(
            "div",
            { class: "bv-row-actions" },
            btn(c.enabled ? "Disable" : "Enable", "ghost sm", async () => {
              await bridge.upsertCommand({ ...c, enabled: !c.enabled });
              await reload();
            }),
            btn("Edit", "ghost sm", () => openCommandModal(c, ctx, refresh)),
            btn("Teach", "ghost sm", async () => {
              try {
                ctx.toast("Recording", `Say "${c.phrase}" once, clearly.`);
                const samples = await captureSamples(1);
                await bridge.trainPhrase(c.phrase, c.actionType === "unlock" ? "unlock" : "command", samples);
                ctx.toast("Phrase taught", `"${c.phrase}" will now be recognised.`);
              } catch (e) {
                ctx.toast("Teaching failed", String(e), "critical");
              }
            }),
            btn("Test", "ghost sm", async () => {
              try {
                const r = await bridge.simulateCommand(c.phrase);
                ctx.toast(r.ok ? "Command matched" : "No match", r.message, r.ok ? "info" : "warn");
              } catch (e) {
                ctx.toast("Test failed", String(e), "critical");
              }
            }),
            btn("Delete", "danger sm", async () => {
              await bridge.deleteCommand(c.id);
              await reload();
            }),
          ),
        ),
      ),
    );
  }

  async function reload(): Promise<void> {
    const cmds = await bridge.listCommands();
    setState({ commands: cmds });
    refresh();
  }

  void reload();
  return section;
}

export function openCommandModal(
  existing: VoiceCommand | null,
  ctx: PaneCtx,
  onSaved: () => void,
): void {
  const phrase = el("input", {
    class: "bv-input",
    placeholder: 'e.g. "next video"',
    value: existing?.phrase ?? "",
  });
  const actionSel = el("select", { class: "bv-select" });
  for (const [k, label] of Object.entries(ACTION_LABELS)) {
    const o = el("option", { value: k }, label);
    if ((existing?.actionType ?? "key_press") === k) o.selected = true;
    actionSel.appendChild(o);
  }
  const payload = el("input", {
    class: "bv-input",
    placeholder: "key combo, url, exe path, app name…",
    value: existing?.payload ?? "",
  });

  let requiresAuth = existing?.requiresAuth ?? false;
  const authToggle = el("div", { class: `bv-switch${requiresAuth ? " on" : ""}`, tabindex: 0 });
  authToggle.addEventListener("click", () => {
    requiresAuth = !requiresAuth;
    authToggle.classList.toggle("on", requiresAuth);
  });

  const scrim = el("div", { class: "bv-scrim" });
  const modal = el(
    "div",
    { class: "bv-modal" },
    el(
      "div",
      { class: "bv-modal-head" },
      el(
        "div",
        {},
        el("h2", { class: "bv-modal-title" }, existing ? "Edit Command" : "New Voice Command"),
        el("div", { class: "bv-modal-desc" }, "Say the phrase exactly. Matching is normalised (case, punctuation and spacing are ignored)."),
      ),
      (() => {
        const b = el("button", { class: "bv-btn icon ghost", type: "button" }, svgIcon(ICONS.close, 16));
        b.addEventListener("click", close);
        return b;
      })(),
    ),
    el("label", { class: "bv-label" }, "Spoken phrase"),
    phrase,
    el("div", { style: "height:14px" }),
    el("label", { class: "bv-label" }, "Action"),
    actionSel,
    el("div", { style: "height:14px" }),
    el("label", { class: "bv-label" }, "Payload (optional)"),
    payload,
    el("div", { class: "bv-hint" }, payloadHint()),
    el("div", { style: "height:16px" }),
    el("div", { style: "display:flex;align-items:center;gap:12px" }, authToggle, el("span", {}, "Require a local factor before running")),
    el("div", { style: "height:20px" }),
    el(
      "div",
      { style: "display:flex;gap:10px;justify-content:flex-end" },
      btn("Cancel", "ghost", close),
      btn("Save Command", "primary", async () => {
        const p = phrase.value.trim();
        if (!p) {
          ctx.toast("Phrase required", "Enter what you will say.", "warn");
          return;
        }
        try {
          await bridge.upsertCommand({
            id: existing?.id,
            phrase: p,
            actionType: actionSel.value as VoiceCommand["actionType"],
            action: actionSel.value,
            payload: payload.value.trim() || null,
            enabled: existing?.enabled ?? true,
            requiresAuth,
            builtin: existing?.builtin ?? false,
          });
          close();
          onSaved();
          ctx.toast("Command saved", `"${p}"`);
        } catch (e) {
          ctx.toast("Save failed", String(e), "critical");
        }
      }),
    ),
  );
  scrim.appendChild(modal);
  scrim.addEventListener("click", (e) => {
    if (e.target === scrim) close();
  });
  document.body.appendChild(scrim);

  function close(): void {
    scrim.remove();
  }
}

function payloadHint(): string {
  return 'key_press → "ctrl+shift+esc" · open_url → "https://…" · launch_app/close_app → exact app name · scroll → "down:600" or "up:300" · mouse_click → leave empty to click the current cursor position';
}

function btn(label: string, variant: string, onClick: () => void | Promise<void>): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant && variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(label));
  b.addEventListener("click", () => void onClick());
  return b;
}
