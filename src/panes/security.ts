// BLACKVAULT — Security pane: the unlock factors.
// Windows Hello, PIN, 3x3 pattern, and the secret passphrase. Everything is
// stored locally; the PIN and pattern are salted-hash only, never plain text.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, relTime } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { getState, setState } from "../lib/store";
import { paneHead, type Pane, type PaneCtx } from "../ui/pane";

export const securityPane: Pane = {
  id: "security",
  title: "Security",
  render(host, ctx) {
    const st = getState();

    const enrollHello = el("button", { class: "bv-btn", type: "button" });
    enrollHello.appendChild(svgIcon(ICONS.fingerprint, 15));
    enrollHello.appendChild(document.createTextNode("Enrol Windows Hello"));
    enrollHello.addEventListener("click", async () => {
      try {
        await bridge.submitHello();
        ctx.toast("Windows Hello", "Verification prompt completed");
        await refreshStatus();
      } catch (e) {
        ctx.toast("Windows Hello unavailable", String(e), "warn");
      }
    });

    mount(
      host,
      paneHead("Authentication Factors", "Local-first · rate limited · audited", enrollHello),
      el(
        "div",
        { class: "bv-grid-cards" },
        helloCard(st, ctx),
        pinCard(st, ctx),
        patternCard(st, ctx),
        passphraseCard(st, ctx),
      ),
      voiceCard(ctx),
    );
  },
};

async function refreshStatus(): Promise<void> {
  try {
    const s = await bridge.getStatus();
    setState({ status: s });
  } catch {
    /* ignore */
  }
}

function helloCard(st = getState(), ctx: PaneCtx): HTMLElement {
  const h = st.hello;
  return card(
    "Windows Hello",
    ICONS.fingerprint,
    h?.available ? (h.enrolled ? "Enrolled" : "Available") : "Unavailable",
    h?.available
      ? h.enrolled
        ? `Method: ${h.method ?? "biometric"} · last ${relTime(h.lastVerified)}`
        : "Windows Hello is present on this device. Enrol it to use fingerprint or PIN as a factor."
      : "This device does not expose Windows Hello. Skip it — PIN and pattern still work.",
    h?.enrolled ? "ok" : "warn",
    ctx,
  );
}

function pinCard(st = getState(), ctx: PaneCtx): HTMLElement {
  const configured = st.status?.pinConfigured ?? false;
  const c = card(
    "PIN",
    ICONS.pin,
    configured ? "Configured" : "Not configured",
    configured ? "Stored as a salted hash. Never written in plain text." : "Set a numeric PIN as a second factor.",
    configured ? "ok" : "warn",
    ctx,
  );
  const actions = el("div", { style: "display:flex;gap:8px;margin-top:12px" });
  actions.appendChild(
    btn(configured ? "Change PIN" : "Set PIN", "default", () => pinModal(ctx)),
  );
  if (configured) actions.appendChild(btn("Clear", "danger", async () => {
    await bridge.clearPin();
    await refreshStatus();
    ctx.toast("PIN cleared");
  }));
  c.appendChild(actions);
  return c;
}

function patternCard(st = getState(), ctx: PaneCtx): HTMLElement {
  const configured = st.status?.patternConfigured ?? false;
  const c = card(
    "Pattern",
    ICONS.grid,
    configured ? "Configured" : "Not configured",
    "3×3 grid, minimum four dots. Stored as a salted hash.",
    configured ? "ok" : "warn",
    ctx,
  );
  const actions = el("div", { style: "display:flex;gap:8px;margin-top:12px" });
  actions.appendChild(btn(configured ? "Change Pattern" : "Set Pattern", "default", () => patternModal(ctx)));
  if (configured) actions.appendChild(btn("Clear", "danger", async () => {
    await bridge.clearPattern();
    await refreshStatus();
    ctx.toast("Pattern cleared");
  }));
  c.appendChild(actions);
  return c;
}

function passphraseCard(st = getState(), ctx: PaneCtx): HTMLElement {
  const configured = st.status?.passphraseConfigured ?? false;
  const c = card(
    "Secret Passphrase",
    ICONS.key,
    configured ? "Configured" : "Not configured",
    "Your spoken secret. Stored as a salted hash; speech is matched to the phrase, then the phrase to the hash.",
    configured ? "ok" : "warn",
    ctx,
  );
  c.appendChild(el("div", { style: "display:flex;gap:8px;margin-top:12px" },
    btn(configured ? "Change" : "Set Passphrase", "default", () => passphraseModal(ctx)),
  ));
  return c;
}

function voiceCard(ctx: PaneCtx): HTMLElement {
  const c = el(
    "div",
    { class: "bv-card", style: "margin-top:16px" },
    el("div", { class: "bv-card-title" }, svgIcon(ICONS.info, 16), "Security model"),
  );
  const bullets = [
    "Speech recognition is a convenience factor, not an identity proof. A protected action always needs a local factor.",
    "Rate limiting: too many failed attempts disable the unlock screen for a cool-down window.",
    "Audio is processed in memory and discarded. Recording to disk is off and cannot be enabled silently.",
    "All secrets live in the local SQLite database, sealed with Windows DPAPI (per-user key).",
  ];
  for (const b of bullets) {
    c.appendChild(
      el("div", { style: "display:flex;gap:10px;margin:9px 0" },
        el("span", { style: "color:var(--ink-faint)" }, "—"),
        el("span", { class: "bv-hint", style: "margin:0" }, b),
      ),
    );
  }
  void ctx;
  return c;
}

// ------------------------------------------------------------------ modals

function pinModal(ctx: PaneCtx): void {
  const pin = el("input", { class: "bv-input", type: "password", inputmode: "numeric", placeholder: "4–12 digits", maxlength: "12" });
  const confirm = el("input", { class: "bv-input", type: "password", inputmode: "numeric", placeholder: "Repeat PIN", maxlength: "12" });
  modal("Set PIN", "Choose a numeric PIN. It is hashed with a per-install salt.", [pin, confirm], async () => {
    const a = pin.value.trim();
    if (!/^\d{4,12}$/.test(a)) {
      ctx.toast("Invalid PIN", "Use 4 to 12 digits.", "warn");
      return false;
    }
    if (a !== confirm.value.trim()) {
      ctx.toast("PINs do not match", "", "warn");
      return false;
    }
    await bridge.setPin(a);
    await refreshStatus();
    ctx.toast("PIN set");
    return true;
  });
}

function passphraseModal(ctx: PaneCtx): void {
  const phrase = el("input", { class: "bv-input", placeholder: "e.g. silver thunder over the vault" });
  const confirm = el("input", { class: "bv-input", placeholder: "Repeat passphrase" });
  modal(
    "Set Secret Passphrase",
    "This is the phrase you will speak to unlock. Use at least four words.",
    [phrase, confirm],
    async () => {
      const a = phrase.value.trim();
      if (a.split(/\s+/).length < 3 || a.length < 10) {
        ctx.toast("Too weak", "Use at least three words and ten characters.", "warn");
        return false;
      }
      if (a !== confirm.value.trim()) {
        ctx.toast("Passphrases do not match", "", "warn");
        return false;
      }
      await bridge.setPassphrase(a);
      await refreshStatus();
      ctx.toast("Passphrase set");
      return true;
    },
  );
}

function patternModal(ctx: PaneCtx): void {
  const grid = el("div", { class: "bv-pattern" });
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "bv-pattern-svg");
  grid.appendChild(svg);
  const nodes: HTMLElement[] = [];
  const chosen: number[] = [];
  const centers: { x: number; y: number }[] = [];

  const redraw = () => {
    while (svg.firstChild) svg.removeChild(svg.firstChild);
    for (let i = 0; i < chosen.length - 1; i++) {
      const a = centers[chosen[i]!]!;
      const b = centers[chosen[i + 1]!]!;
      const l = document.createElementNS("http://www.w3.org/2000/svg", "line");
      l.setAttribute("x1", String(a.x)); l.setAttribute("y1", String(a.y));
      l.setAttribute("x2", String(b.x)); l.setAttribute("y2", String(b.y));
      svg.appendChild(l);
    }
  };

  for (let i = 0; i < 9; i++) {
    const n = el("div", { class: "bv-pattern-node", "data-i": i });
    n.addEventListener("pointerenter", (e) => {
      if (!(e as PointerEvent).buttons || chosen.includes(i)) return;
      chosen.push(i); n.classList.add("on"); redraw();
    });
    n.addEventListener("pointerdown", () => {
      chosen.length = 0;
      nodes.forEach((x) => x.classList.remove("on"));
      chosen.push(i); n.classList.add("on"); redraw();
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

  modal(
    "Set Pattern",
    "Draw a pattern by dragging across at least four dots.",
    [el("div", { style: "display:flex;justify-content:center" }, grid)],
    async () => {
      if (chosen.length < 4) {
        ctx.toast("Pattern too short", "Connect at least four dots.", "warn");
        return false;
      }
      await bridge.setPattern(chosen.slice());
      await refreshStatus();
      ctx.toast("Pattern set");
      return true;
    },
  );
}

function modal(
  title: string,
  desc: string,
  fields: HTMLElement[],
  onSave: () => Promise<boolean>,
): void {
  const scrim = el("div", { class: "bv-scrim" });
  const saveBtn = btn("Save", "primary", async () => {
    try {
      if (await onSave()) scrim.remove();
    } catch (e) {
      void e;
    }
  });
  const box = el(
    "div",
    { class: "bv-modal" },
    el(
      "div",
      { class: "bv-modal-head" },
      el("div", {}, el("h2", { class: "bv-modal-title" }, title), el("div", { class: "bv-modal-desc" }, desc)),
      (() => { const b = el("button", { class: "bv-btn icon ghost", type: "button" }, svgIcon(ICONS.close, 16)); b.addEventListener("click", () => scrim.remove()); return b; })(),
    ),
    ...fields.flatMap((f) => [el("div", { style: "margin-bottom:14px" }, f)]),
    el("div", { style: "display:flex;gap:10px;justify-content:flex-end;margin-top:8px" },
      btn("Cancel", "ghost", () => scrim.remove()),
      saveBtn,
    ),
  );
  scrim.appendChild(box);
  scrim.addEventListener("click", (e) => { if (e.target === scrim) scrim.remove(); });
  document.body.appendChild(scrim);
}

function card(
  title: string,
  icon: string,
  value: string,
  note: string,
  tone: string,
  _ctx: PaneCtx,
): HTMLElement {
  return el(
    "div",
    { class: "bv-card" },
    el("div", { class: "bv-card-title" }, svgIcon(icon, 16), title),
    el("div", { class: "bv-metric-sm", style: "display:flex;align-items:center;gap:8px" },
      el("span", { class: `bv-dot ${tone === "ok" ? "on" : "warn"}` }),
      value,
    ),
    el("div", { class: "bv-metric-note" }, note),
  );
}

function btn(text: string, variant: string, onClick: () => void | Promise<void>): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant && variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(text));
  b.addEventListener("click", () => void onClick());
  return b;
}
