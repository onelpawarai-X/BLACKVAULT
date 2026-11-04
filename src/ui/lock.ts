// BLACKVAULT — Full-Screen Security Mode.
//
// Two very different screens live here:
//   * the interactive lock (primary monitor): the lock ceremony, live listening
//     read-out, the multi-factor challenge, and the unlock ceremony;
//   * the passive overlay (every other monitor): pure black, a faint emblem,
//     nothing focusable — so a second display can never leak the desktop.
//
// The whole thing is driven by core events; it holds no security state itself.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, clear } from "../lib/dom";
import { ICONS } from "../lib/icons";
import type { AuthResult } from "../lib/types";

export interface LockController {
  show(reason?: string): void;
  hide(): void;
  setHeard(text: string, isFinal: boolean): void;
  setChallenge(factors: string[]): void;
  /** Run the unlock ceremony. `token` is the core-issued session token. */
  playUnlock(ok: boolean, message: string, token?: string): void;
  setLevel(level: number): void;
}

/** True when this window is one of the extra monitor overlays. */
export function isOverlayWindow(): boolean {
  return new URLSearchParams(location.search).get("overlay") === "1";
}

/** Render the passive overlay for a secondary monitor. */
export function renderPassiveOverlay(host: HTMLElement): void {
  const svg = svgIcon(ICONS.vault, 120);
  svg.setAttribute("stroke-width", "0.6");
  svg.style.opacity = "0.10";
  mount(
    host,
    el(
      "div",
      {
        class: "bv-lock",
        style:
          "flex-direction:column;gap:24px;background:#000;cursor:none;",
      },
      el("div", { style: "position:relative;display:grid;place-items:center" }, svg, halo()),
      el(
        "div",
        {
          class: "bv-lock-title",
          style: "color:rgba(255,255,255,0.30);font-size:12px",
        },
        "BLACKVAULT SECURE DISPLAY",
      ),
    ),
  );
}

function halo(): HTMLElement {
  const h = el("div", { class: "halo" });
  h.style.cssText =
    "position:absolute;inset:-60px;border-radius:50%;background:radial-gradient(circle,rgba(255,255,255,0.06),transparent 62%);animation:breathe 5s ease-in-out infinite";
  return h;
}

export function createLockScreen(host: HTMLElement): LockController {
  const root = el("div", { class: "bv-lock", style: "display:none" });
  host.appendChild(root);

  let phase: "hidden" | "locking" | "locked" | "unlocking" = "hidden";
  let factors: string[] = [];
  let attemptsNote = "";
  let levelBars: HTMLElement[] = [];

  // ---------------------------------------------------------------- locking
  function playLocking(reason?: string): void {
    phase = "locking";
    root.style.display = "flex";
    root.className = "bv-lock bv-lock-locking";

    const top = el("div", { class: "bv-iris-top" });
    const bottom = el("div", { class: "bv-iris-bottom" });
    const line = el("div", { class: "bv-iris-line" });
    const shock = el("div", { class: "bv-shock" });
    const scan = el("div", { class: "bv-scan" });

    mount(
      root,
      el("div", { class: "bv-iris" }, top, bottom, line, shock, scan),
      el(
        "div",
        { class: "bv-lock-core" },
        el(
          "div",
          { class: "bv-lock-title", style: "color:rgba(255,255,255,0.55)" },
          "SECURING",
        ),
      ),
    );

    // The iris closes over ~900ms; once shut, swap to the locked face.
    window.setTimeout(() => {
      if (phase === "locking") renderLocked(reason);
    }, 940);
  }

  // ----------------------------------------------------------------- locked
  function renderLocked(reason?: string): void {
    phase = "locked";
    root.className = "bv-lock";
    // The core knows which factors are configured; ask it so the passphrase
    // prompt is present even when the user never speaks the unlock phrase.
    void bridge
      .unlockFactors()
      .then((f) => {
        if (phase === "locked" && f.length) {
          factors = f;
          const ch = root.querySelector('[data-role="challenge"]') as HTMLElement | null;
          if (ch) renderChallenge(ch);
        }
      })
      .catch(() => undefined);

    const emblem = el(
      "div",
      { class: "bv-lock-emblem" },
      halo(),
      svgIcon(ICONS.lock, 78),
    );
    // Lock body accent: white, so the emblem reads as "sealed".
    (emblem.querySelector("svg") as SVGElement | null)?.setAttribute("stroke-width", "1.2");

    levelBars = [];
    const wave = el("div", { class: "bv-wave is-live" });
    for (let i = 0; i < 14; i++) {
      const b = el("span", {});
      b.style.animationDelay = `${(i * 70) % 700}ms`;
      wave.appendChild(b);
      levelBars.push(b);
    }

    const heard = el("div", { class: "bv-lock-heard", "data-role": "heard" }, "");
    const challenge = el("div", { class: "bv-lock-challenge", "data-role": "challenge" });

    mount(
      root,
      el(
        "div",
        { class: "bv-lock-core enter" },
        emblem,
        el("div", { class: "bv-lock-title" }, "VAULT LOCKED"),
        el(
          "div",
          { class: "bv-lock-sub" },
          reason
            ? `Desktop hidden · ${reason}`
            : "Desktop hidden · background tasks keep running",
        ),
        el(
          "div",
          { class: "bv-lock-listen" },
          svgIcon(ICONS.mic, 15),
          el("span", { class: "label" }, "Listening for unlock"),
          wave,
        ),
        heard,
        challenge,
      ),
    );

    renderChallenge(challenge);
  }

  function renderChallenge(host2: HTMLElement): void {
    clear(host2);
    const hasPass = factors.includes("passphrase");
    const hasPin = factors.includes("pin");
    const hasPattern = factors.includes("pattern");
    const hasHello = factors.includes("hello");
    const hasVoice = factors.includes("voice");

    // Step one offers the passphrase (spoken or typed); step two offers the
    // remaining factors only, so the wording has to follow which step we are on.
    const firstStep = hasPass || hasVoice;
    const introText = firstStep
      ? "Speak your unlock phrase" + (hasPin || hasPattern || hasHello ? " · or use a factor below" : "")
      : "Second factor required";
    const intro = el(
      "div",
      { class: "bv-hint", style: "text-align:center;letter-spacing:0.16em;text-transform:uppercase" },
      introText,
    );
    host2.appendChild(intro);

    if (attemptsNote) {
      host2.appendChild(
        el(
          "div",
          { class: "bv-hint", style: "text-align:center;color:var(--danger)" },
          attemptsNote,
        ),
      );
    }

    if (hasPass) {
      const input = el("input", {
        class: "bv-input",
        type: "password",
        placeholder: "PASSPHRASE",
        "aria-label": "Passphrase",
      });
      const send = () => {
        const v = input.value.trim();
        if (!v) return;
        input.value = "";
        void attempt("passphrase", v);
      };
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") send();
      });
      host2.appendChild(input);
      host2.appendChild(
        el(
          "div",
          { style: "display:flex;gap:10px" },
          btn("Verify Passphrase", "primary", send),
          hasHello ? btn("Windows Hello", "default", () => void attempt("hello", "")) : null,
        ),
      );
    } else if (hasHello) {
      host2.appendChild(
        el(
          "div",
          { style: "display:flex;gap:10px;justify-content:center" },
          btn("Windows Hello", "primary", () => void attempt("hello", "")),
        ),
      );
    }

    if (hasPin) {
      const pin = el("input", {
        class: "bv-input",
        type: "password",
        placeholder: "PIN",
        inputmode: "numeric",
        "aria-label": "PIN",
      });
      pin.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          const v = pin.value.trim();
          pin.value = "";
          if (v) void attempt("pin", v);
        }
      });
      host2.appendChild(pin);
    }

    if (hasPattern) {
      host2.appendChild(buildPattern(host2));
    }

    if (hasVoice) {
      host2.appendChild(
        el(
          "div",
          { class: "bv-hint", style: "text-align:center" },
          "Voice identity is checked automatically against your enrolled phrase.",
        ),
      );
    }
  }

  function buildPattern(parent: HTMLElement): HTMLElement {
    const wrap = el("div", { style: "display:flex;flex-direction:column;align-items:center;gap:12px" });
    const grid = el("div", { class: "bv-pattern" });
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("class", "bv-pattern-svg");
    grid.appendChild(svg);
    const nodes: HTMLElement[] = [];
    const chosen: number[] = [];
    const centers: { x: number; y: number }[] = [];

    const redraw = () => {
      clear(svg);
      for (let i = 0; i < chosen.length - 1; i++) {
        const a = centers[chosen[i]!]!;
        const b = centers[chosen[i + 1]!]!;
        const l = document.createElementNS("http://www.w3.org/2000/svg", "line");
        l.setAttribute("x1", String(a.x));
        l.setAttribute("y1", String(a.y));
        l.setAttribute("x2", String(b.x));
        l.setAttribute("y2", String(b.y));
        svg.appendChild(l);
      }
    };

    for (let i = 0; i < 9; i++) {
      const n = el("div", { class: "bv-pattern-node", "data-i": i });
      n.addEventListener("pointerenter", (e) => {
        if (!(e as PointerEvent).buttons) return;
        if (chosen.includes(i)) return;
        chosen.push(i);
        n.classList.add("on");
        redraw();
      });
      n.addEventListener("pointerdown", () => {
        chosen.length = 0;
        nodes.forEach((x) => x.classList.remove("on"));
        chosen.push(i);
        n.classList.add("on");
        redraw();
      });
      grid.appendChild(n);
      nodes.push(n);
    }

    const finish = () => {
      if (chosen.length < 4) {
        attemptsNote = "Pattern must connect at least 4 dots.";
        renderChallenge(parent);
        return;
      }
      void attempt("pattern", chosen.join("-"));
    };
    grid.addEventListener("pointerup", finish);
    grid.addEventListener("pointerleave", () => {
      if (chosen.length >= 4) finish();
    });

    // Centers are measured after layout.
    requestAnimationFrame(() => {
      const gr = grid.getBoundingClientRect();
      nodes.forEach((n) => {
        const r = n.getBoundingClientRect();
        centers.push({
          x: r.left - gr.left + r.width / 2,
          y: r.top - gr.top + r.height / 2,
        });
      });
    });

    wrap.appendChild(grid);
    wrap.appendChild(btn("Clear", "ghost", () => {
      chosen.length = 0;
      nodes.forEach((x) => x.classList.remove("on"));
      redraw();
    }));
    return wrap;
  }

  async function attempt(method: string, value: string): Promise<void> {
    try {
      let res: AuthResult;
      if (method === "passphrase") res = await bridge.submitPassphrase(value);
      else if (method === "pin") res = await bridge.submitPin(value);
      else if (method === "pattern") res = await bridge.submitPattern(value.split("-").map(Number));
      else if (method === "hello") res = await bridge.submitHello();
      else res = await bridge.submitPassphrase(value);

      if (!res.ok) {
        attemptsNote =
          res.remainingAttempts !== null
            ? `${res.message} · ${res.remainingAttempts} attempt(s) left`
            : res.message;
        root.classList.add("bv-shake");
        setTimeout(() => root.classList.remove("bv-shake"), 500);
        const ch = root.querySelector('[data-role="challenge"]') as HTMLElement | null;
        if (ch) renderChallenge(ch);
      }
    } catch (err) {
      attemptsNote = String(err);
      const ch = root.querySelector('[data-role="challenge"]') as HTMLElement | null;
      if (ch) renderChallenge(ch);
    }
  }

  // --------------------------------------------------------------- unlocking
  function playUnlock(ok: boolean, message: string, token?: string): void {
    if (phase !== "locked" && phase !== "unlocking") return;
    phase = "unlocking";

    const variant = document.documentElement.dataset.unlockAnim ?? "bloom";
    const bloom = el("div", { class: "bv-unlock-bloom" });
    const rings = el(
      "div",
      { class: "bv-unlock-rings" },
      el("i", {}),
      el("i", {}),
      el("i", {}),
    );
    const sweep = el("div", { class: "bv-unlock-sweep" });

    const glyph = svgIcon(ok ? ICONS.unlock : ICONS.lock, 84);
    glyph.setAttribute("stroke-width", "1.1");

    const anim = el(
      "div",
      { class: "bv-unlock-anim" },
      variant === "rings" ? rings : variant === "vortex" ? el("div", { class: "bv-unlock-vortex" }, rings) : bloom,
      sweep,
      el(
        "div",
        { class: "bv-unlock-glyph" },
        glyph,
        el("div", { class: "bv-unlock-word" }, ok ? "ACCESS GRANTED" : message),
      ),
    );
    root.appendChild(anim);

    window.setTimeout(() => {
      anim.remove();
      if (ok) {
        // The core already completed the unlock and issued this token; present
        // it so the core clears the token, then drop the overlay.
        void bridge
          .completeUnlock(token ?? "session")
          .catch(() => undefined)
          .finally(() => forceHide());
      } else {
        phase = "locked";
      }
    }, 1650);
  }

  // ------------------------------------------------------------------ misc
  function setHeard(text: string, isFinal: boolean): void {
    const n = root.querySelector('[data-role="heard"]') as HTMLElement | null;
    if (n) n.textContent = isFinal ? `"${text}"` : text;
  }

  function setChallenge(f: string[]): void {
    factors = f;
    const ch = root.querySelector('[data-role="challenge"]') as HTMLElement | null;
    if (ch && phase === "locked") renderChallenge(ch);
  }

  function setLevel(level: number): void {
    if (!levelBars.length) return;
    const active = Math.round(level * levelBars.length);
    levelBars.forEach((b, i) => {
      b.style.height = i < active ? `${30 + level * 70}%` : "18%";
    });
  }

  function show(reason?: string): void {
    attemptsNote = "";
    playLocking(reason);
  }

  function hide(): void {
    // While the unlock ceremony is playing it owns the teardown. The core emits
    // "mode: unlocked" before "unlocked: {token}", so a plain hide here would
    // wipe the overlay before the animation had run.
    if (phase === "unlocking") return;
    forceHide();
  }

  function forceHide(): void {
    phase = "hidden";
    root.style.display = "none";
    clear(root);
  }

  return { show, hide, setHeard, setChallenge, playUnlock, setLevel };
}

function btn(label: string, variant: string, onClick: () => void): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(label));
  b.addEventListener("click", onClick);
  return b;
}
