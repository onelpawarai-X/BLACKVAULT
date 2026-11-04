// BLACKVAULT — First-run Security Setup Wizard.
//
// Runs once, on the first launch. It is skippable at every step: if this device
// has no Windows Hello, or the user simply does not want a PIN or pattern, the
// wizard moves on. What it will not do is pretend a factor is configured when it
// is not — the dashboard always reflects the truth.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { getState, setState } from "../lib/store";
import type { Settings } from "../lib/types";

const STEPS = ["Identity", "Passphrase", "PIN", "Pattern", "Windows Hello", "Voice", "Recognition", "Startup"] as const;

export function createWizard(host: HTMLElement, onDone: () => void): void {
  let step = 0;
  const draft: Partial<Settings> = {};
  let pattern: number[] = [];
  let voiceSamples: number[] = [];

  const scrim = el("div", { class: "bv-scrim", style: "z-index:80" });
  const body = el("div", {});
  const stepDots = el("div", { style: "display:flex;gap:6px;justify-content:center;margin-bottom:18px" });
  const box = el(
    "div",
    { class: "bv-modal", style: "width:min(600px,94vw)" },
    el(
      "div",
      { style: "display:flex;flex-direction:column;align-items:center;margin-bottom:8px" },
      (() => { const s = svgIcon(ICONS.vault, 40); s.setAttribute("stroke-width", "1.2"); return s; })(),
      el("div", { class: "bv-modal-title", style: "margin-top:12px;letter-spacing:0.2em;text-transform:uppercase;font-size:14px" }, "Security Setup"),
      el("div", { class: "bv-modal-desc", style: "text-align:center;max-width:420px" }, "Set up how BLACKVAULT recognises you. Skip anything you do not want — you can change it later in Settings."),
    ),
    stepDots,
    body,
  );
  scrim.appendChild(box);
  host.appendChild(scrim);

  function drawDots(): void {
    mount(stepDots, ...STEPS.map((_, i) => {
      const d = el("span", {});
      d.style.cssText = `width:${i === step ? 22 : 6}px;height:6px;border-radius:999px;background:${i <= step ? "rgba(255,255,255,0.85)" : "rgba(255,255,255,0.15)"};transition:all .3s ease`;
      return d;
    }));
  }

  function next(): void {
    step++;
    if (step >= STEPS.length) {
      void finish();
      return;
    }
    draw();
  }
  function back(): void {
    if (step > 0) step--;
    draw();
  }

  function nav(extra?: HTMLElement): HTMLElement {
    return el(
      "div",
      { style: "display:flex;gap:10px;justify-content:space-between;margin-top:22px" },
      el("div", {}, step > 0 ? btn("Back", "ghost", back) : null),
      el(
        "div",
        { style: "display:flex;gap:10px" },
        extra,
        btn(step === STEPS.length - 1 ? "Finish" : "Continue", "primary", next),
        btn("Skip all", "ghost", () => void finish()),
      ),
    );
  }

  function draw(): void {
    drawDots();
    const title = STEPS[step]!;
    switch (title) {
      case "Identity": return stepIdentity();
      case "Passphrase": return stepPassphrase();
      case "PIN": return stepPin();
      case "Pattern": return stepPattern();
      case "Windows Hello": return stepHello();
      case "Recognition": return stepRecognition();
      case "Voice": return stepVoice();
      case "Startup": return stepStartup();
    }
  }

  // -------------------------------------------------------------- steps
  function stepIdentity(): void {
    const name = el("input", {
      class: "bv-input",
      placeholder: "e.g. Arin",
      value: draft.ownerName ?? "",
    });
    // Persisted with the rest of the draft so the lock screen can greet the
    // user by name instead of showing a bare "VAULT LOCKED".
    name.addEventListener("input", () => {
      draft.ownerName = name.value.trim();
    });
    mount(
      body,
      field("Your name", name, "Shown on the lock screen and used to label your voice profile. Never transmitted anywhere."),
      nav(),
    );
  }

  function stepPassphrase(): void {
    const p1 = el("input", { class: "bv-input", placeholder: "e.g. silver thunder over the vault" });
    const p2 = el("input", { class: "bv-input", placeholder: "Repeat passphrase" });
    const note = el("div", { class: "bv-hint" });
    const set = btn("Set passphrase", "default", async () => {
      const a = p1.value.trim();
      if (a.split(/\s+/).length < 3 || a.length < 10) {
        note.textContent = "Use at least three words and ten characters.";
        return;
      }
      if (a !== p2.value.trim()) {
        note.textContent = "The two passphrases do not match.";
        return;
      }
      try {
        await bridge.setPassphrase(a);
        note.textContent = "Passphrase set. Speak this phrase to unlock.";
        note.style.color = "var(--ok)";
      } catch (e) {
        note.textContent = String(e);
      }
    });
    mount(
      body,
      field("Primary unlock passphrase", p1, "This is what you will say when the vault is locked."),
      field("Confirm", p2),
      el("div", { style: "display:flex;gap:10px;align-items:center" }, set, note),
      nav(),
    );
  }

  function stepPin(): void {
    const pin = el("input", { class: "bv-input", type: "password", inputmode: "numeric", maxlength: "12", placeholder: "4–12 digits" });
    const confirm = el("input", { class: "bv-input", type: "password", inputmode: "numeric", maxlength: "12", placeholder: "Repeat PIN" });
    const note = el("div", { class: "bv-hint" });
    const set = btn("Set PIN", "default", async () => {
      const a = pin.value.trim();
      if (!/^\d{4,12}$/.test(a)) { note.textContent = "Use 4 to 12 digits."; return; }
      if (a !== confirm.value.trim()) { note.textContent = "The PINs do not match."; return; }
      try {
        await bridge.setPin(a);
        note.textContent = "PIN set.";
        note.style.color = "var(--ok)";
      } catch (e) { note.textContent = String(e); }
    });
    mount(
      body,
      field("PIN", pin, "A local second factor. Stored as a salted hash, never as plain text."),
      field("Confirm", confirm),
      el("div", { style: "display:flex;gap:10px;align-items:center" }, set, note),
      nav(),
    );
  }

  function stepPattern(): void {
    const grid = el("div", { class: "bv-pattern" });
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("class", "bv-pattern-svg");
    grid.appendChild(svg);
    const nodes: HTMLElement[] = [];
    const centers: { x: number; y: number }[] = [];
    const redraw = () => {
      while (svg.firstChild) svg.removeChild(svg.firstChild);
      for (let i = 0; i < pattern.length - 1; i++) {
        const a = centers[pattern[i]!]!;
        const b = centers[pattern[i + 1]!]!;
        const l = document.createElementNS("http://www.w3.org/2000/svg", "line");
        l.setAttribute("x1", String(a.x)); l.setAttribute("y1", String(a.y));
        l.setAttribute("x2", String(b.x)); l.setAttribute("y2", String(b.y));
        svg.appendChild(l);
      }
    };
    for (let i = 0; i < 9; i++) {
      const n = el("div", { class: "bv-pattern-node" });
      n.addEventListener("pointerenter", (e) => {
        if (!(e as PointerEvent).buttons || pattern.includes(i)) return;
        pattern.push(i); n.classList.add("on"); redraw();
      });
      n.addEventListener("pointerdown", () => {
        pattern = [];
        nodes.forEach((x) => x.classList.remove("on"));
        pattern.push(i); n.classList.add("on"); redraw();
      });
      grid.appendChild(n);
      nodes.push(n);
    }
    requestAnimationFrame(() => {
      const gr = grid.getBoundingClientRect();
      nodes.forEach((n) => {
        const r = n.getBoundingClientRect();
        centers.push({ x: r.left - gr.left + r.width / 2, y: r.top - gr.top + r.height / 2 });
      });
    });
    const note = el("div", { class: "bv-hint" });
    const set = btn("Set pattern", "default", async () => {
      if (pattern.length < 4) { note.textContent = "Connect at least four dots."; return; }
      try {
        await bridge.setPattern(pattern.slice());
        note.textContent = "Pattern set.";
        note.style.color = "var(--ok)";
      } catch (e) { note.textContent = String(e); }
    });
    mount(
      body,
      el("div", { class: "bv-label" }, "Draw a pattern (drag across at least four dots)"),
      el("div", { style: "display:flex;justify-content:center;margin:10px 0" }, grid),
      el("div", { style: "display:flex;gap:10px;align-items:center;justify-content:center" }, set, note),
      nav(),
    );
  }

  function stepHello(): void {
    const h = getState().hello;
    const note = el("div", { class: "bv-hint" });
    const test = btn("Test Windows Hello", "default", async () => {
      try {
        const r = await bridge.submitHello();
        note.textContent = r.ok ? "Verified. Windows Hello is ready to use." : r.message;
        note.style.color = r.ok ? "var(--ok)" : "var(--ink-faint)";
      } catch (e) {
        note.textContent = "Windows Hello is not available on this device. You can skip this step.";
        note.style.color = "var(--ink-faint)";
      }
    });
    mount(
      body,
      el("div", { class: "bv-hint", style: "margin-bottom:12px" },
        h?.available
          ? "Windows Hello is available on this device. You can use fingerprint, face or your Windows PIN as a factor."
          : "Windows Hello does not appear to be available here. This step is optional — skip it and rely on your PIN, pattern or passphrase."),
      el("div", { style: "display:flex;gap:10px;align-items:center" }, test, note),
      nav(),
    );
  }

  function stepVoice(): void {
    const wave = el("div", { class: "bv-wave" });
    for (let i = 0; i < 24; i++) wave.appendChild(el("span", {}));
    const note = el("div", { class: "bv-hint" }, "Optional. Enrol your voice so the unlock screen can recognise you. Audio is processed in memory and never saved.");
    const rec = btn("Record voice sample", "default", async () => {
      try {
        voiceSamples = await capture(2);
        note.textContent = "Samples captured. Enrolling…";
        await bridge.enrollVoice(voiceSamples);
        note.textContent = "Voice identity enrolled.";
        note.style.color = "var(--ok)";
      } catch (e) {
        note.textContent = `Could not capture voice: ${String(e)}`;
      }
    });
    mount(body, el("div", { class: "bv-label" }, "Voice identity"), el("div", { style: "margin:10px 0" }, wave), el("div", { style: "display:flex;gap:10px;align-items:center" }, rec), note, nav());
  }

  // The step that makes the difference between a demo and a working
  // installation: confirm the speech service actually answers before the user
  // leaves setup, so they are never left wondering why nothing is recognised.
  function stepRecognition(): void {
    let lang = draft.sttLanguage ?? "both";
    const note = el(
      "div",
      { class: "bv-hint" },
      "BLACKVAULT recognises speech through your own endpoint. Nothing is stored; audio is sent for recognition and discarded.",
    );
    const sel = el("select", { class: "bv-select" });
    for (const [v, l] of [
      ["en", "English"],
      ["bn", "Bengali"],
      ["both", "Both (English + Bengali)"],
    ] as [string, string][]) {
      const o = el("option", { value: v }, l);
      if (v === lang) o.selected = true;
      sel.appendChild(o);
    }
    sel.addEventListener("change", () => {
      lang = sel.value;
      draft.sttLanguage = sel.value;
    });
    const test = btn("Test recognition service", "default", async () => {
      note.textContent = "Contacting the endpoint\u2026";
      const current = getState().settings;
      if (!current) return;
      try {
        const t = await bridge.probeStt({ ...current, sttLanguage: lang, sttEnabled: true });
        note.textContent = `Reachable \u00b7 ${t.locale || t.engine || "ok"} \u00b7 recognised ${t.text ? `\u201c${t.text}\u201d` : "silence"}`;
        note.style.color = "var(--ok)";
      } catch (e) {
        note.textContent = `Could not reach the service: ${String(e)}. You can continue and fix this in Settings.`;
        note.style.color = "var(--warn)";
      }
    });
    mount(
      body,
      el("div", { class: "bv-label" }, "Speech recognition"),
      field("Language", sel, "Pick Both if you speak English and Bengali in the same session."),
      el("div", { style: "display:flex;gap:10px;align-items:center;margin:10px 0" }, test),
      note,
      nav(),
    );
  }

  function stepStartup(): void {
    const startWin = el("div", { class: "bv-switch on", tabindex: 0 });
    const startMin = el("div", { class: "bv-switch", tabindex: 0 });
    const alwaysOn = el("div", { class: "bv-switch on", tabindex: 0 });
    let sw = true, sm = false, ao = true;
    startWin.addEventListener("click", () => { sw = !sw; startWin.classList.toggle("on", sw); });
    startMin.addEventListener("click", () => { sm = !sm; startMin.classList.toggle("on", sm); });
    alwaysOn.addEventListener("click", () => { ao = !ao; alwaysOn.classList.toggle("on", ao); });
    (draft as { _sw?: boolean })._sw = true;
    mount(
      body,
      toggleRow("Start with Windows", "Launch BLACKVAULT at sign-in so the voice engine is always armed.", startWin),
      toggleRow("Start minimised", "Begin in the tray.", startMin),
      toggleRow("Always-on voice listening", "Keep the microphone armed while unlocked.", alwaysOn),
      nav(btn("Finish", "primary", () => {
        draft.startWithWindows = sw;
        draft.startMinimized = sm;
        draft.alwaysOnVoice = ao;
        void finish();
      })),
    );
  }

  // ------------------------------------------------------------- finishing
  async function finish(): Promise<void> {
    try {
      const next = await bridge.saveSettings({ ...getState().settings!, ...draft, setupComplete: true });
      setState({ settings: next });
      if (draft.startWithWindows) {
        await bridge.setAutostart(true).catch(() => undefined);
      }
    } catch {
      /* non-fatal */
    }
    scrim.remove();
    onDone();
  }

  draw();
}

function field(label: string, control: HTMLElement, hint?: string): HTMLElement {
  return el(
    "div",
    { style: "margin-bottom:14px" },
    el("label", { class: "bv-label" }, label),
    control,
    hint ? el("div", { class: "bv-hint" }, hint) : null,
  );
}

function toggleRow(label: string, note: string, control: HTMLElement): HTMLElement {
  return el(
    "div",
    { class: "bv-row", style: "border:none" },
    el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, label), el("div", { class: "bv-row-meta" }, note)),
    control,
  );
}

function btn(text: string, variant: string, onClick: () => void | Promise<void>): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant && variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(text));
  b.addEventListener("click", () => void onClick());
  return b;
}

async function capture(n: number): Promise<number[]> {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const ctx = new AudioContext();
  const src = ctx.createMediaStreamSource(stream);
  const analyser = ctx.createAnalyser();
  analyser.fftSize = 2048;
  src.connect(analyser);
  const buf = new Float32Array(analyser.fftSize);
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const frames: number[] = [];
    const t0 = performance.now();
    while (performance.now() - t0 < 800) {
      analyser.getFloatTimeDomainData(buf);
      for (let k = 0; k < buf.length; k += 4) frames.push(buf[k]!);
      await new Promise((r) => setTimeout(r, 40));
    }
    out.push(...frames.slice(0, 512));
    await new Promise((r) => setTimeout(r, 300));
  }
  stream.getTracks().forEach((t) => t.stop());
  await ctx.close();
  return out;
}
