// BLACKVAULT — the authentication prompt window.
//
// This is the window that appears when a protected application starts, or when
// a protected file or folder is opened. It is deliberately its own window: the
// control centre lives in the tray while the user works, so a card drawn into it
// would never be seen, and "the lock does not ask for a password" was exactly
// that bug.
//
// The window holds no security state. It receives one prompt, forwards the
// answer to the core, and closes.

import "../styles/prompt.css";

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, clear } from "../lib/dom";
import { ICONS } from "../lib/icons";
import type { AuthResult } from "../lib/types";

interface PromptInfo {
  token: string;
  kind: "app" | "path";
  title: string;
  name: string;
  detail: string;
  origin?: string | null;
}

export function renderPrompt(host: HTMLElement): void {
  document.body.classList.add("bv-prompt-body");
  let info: PromptInfo | null = null;
  let busy = false;
  let note = "";

  // The core may emit before this page has finished loading, and it emits twice
  // for exactly that reason. Whichever copy arrives first wins; the second is
  // ignored once a prompt is on screen.
  bridge.onCoreEvent((e) => {
    if (e.type === "appChallenge") {
      if (info) return;
      const p = e.payload;
      info = {
        token: p.token,
        kind: "app",
        title: "Application blocked",
        name: p.displayName || p.processName,
        detail: `${p.processName} started without authorisation. Authenticate to let it run, or close it.`,
        origin: p.origin,
      };
      render();
    } else if (e.type === "pathChallenge") {
      if (info) return;
      const p = e.payload;
      info = {
        token: p.token,
        kind: "path",
        title: p.kind === "folder" ? "Protected folder opened" : "Protected file opened",
        name: p.displayName,
        detail: p.path,
        origin: p.origin,
      };
      render();
    } else if (e.type === "promptClosed") {
      window.close();
    }
  });

  render();

  function render(): void {
    clear(host);
    if (!info) {
      host.appendChild(
        el(
          "div",
          { class: "bv-prompt wait" },
          el("div", { class: "bv-prompt-spin" }),
          el("div", { class: "bv-prompt-wait-text" }, "Waiting for the security core"),
        ),
      );
      return;
    }

    const emblem = el("div", { class: "bv-prompt-emblem" }, svgIcon(ICONS.shield, 30));
    const heading = el(
      "div",
      { class: "bv-prompt-head" },
      emblem,
      el(
        "div",
        {},
        el("div", { class: "bv-prompt-title" }, info.title),
        el("div", { class: "bv-prompt-name" }, info.name),
      ),
    );

    const body = el(
      "div",
      { class: "bv-prompt-body" },
      el("div", { class: "bv-prompt-detail" }, info.detail),
      info.origin ? el("div", { class: "bv-prompt-origin" }, `Requested by ${shortPath(info.origin)}`) : null,
    );

    const pin = el("input", {
      class: "bv-input",
      type: "password",
      placeholder: "PIN OR PASSWORD",
      autocomplete: "off",
      "aria-label": "PIN or password",
    });
    pin.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") void answer(pin.value, "");
    });

    const phrase = el("input", {
      class: "bv-input",
      type: "password",
      placeholder: "SECRET PASSPHRASE (optional)",
      autocomplete: "off",
      "aria-label": "Secret passphrase",
    });
    phrase.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") void answer(pin.value, phrase.value);
    });

    const actions = el(
      "div",
      { class: "bv-prompt-actions" },
      primaryBtn("Authenticate", () => void answer(pin.value, phrase.value)),
      ghostBtn("Windows Hello", () => void hello()),
      ghostBtn(info.kind === "app" ? "Close it" : "Keep sealed", () => void deny()),
    );

    const noteEl = el("div", { class: "bv-prompt-note" }, note);

    mount(
      host,
      el(
        "div",
        { class: "bv-prompt enter" },
        heading,
        body,
        el("div", { class: "bv-prompt-field" }, el("div", { class: "bv-prompt-label" }, "Password or PIN"), pin),
        el(
          "div",
          { class: "bv-prompt-field" },
          el("div", { class: "bv-prompt-label" }, "Or your spoken secret phrase"),
          phrase,
        ),
        noteEl,
        actions,
        el(
          "div",
          { class: "bv-prompt-foot" },
          "Verified locally with PBKDF2. Nothing is sent anywhere.",
        ),
      ),
    );
    pin.focus();
  }

  async function answer(pin: string, phrase: string): Promise<void> {
    if (!info || busy) return;
    if (!pin.trim() && !phrase.trim()) {
      note = "Enter your PIN or your secret passphrase.";
      render();
      return;
    }
    busy = true;
    try {
      const res = await bridge.answerPrompt(info.token, pin, phrase);
      handle(res);
    } catch (err) {
      busy = false;
      note = String(err);
      render();
    }
  }

  async function hello(): Promise<void> {
    if (!info || busy) return;
    busy = true;
    try {
      const res = await bridge.answerPrompt(info.token, "", "");
      handle(res);
    } catch (err) {
      busy = false;
      note = String(err);
      render();
    }
  }

  function handle(res: AuthResult): void {
    if (!res.ok) {
      busy = false;
      note = res.message || "That password was not accepted.";
      render();
      const card = host.querySelector(".bv-prompt") as HTMLElement | null;
      card?.classList.add("shake");
      window.setTimeout(() => card?.classList.remove("shake"), 480);
      return;
    }
    // Accepted. Play the release ceremony, then hand the item to its own
    // application so the user sees the thing they asked for.
    if (info) void bridge.openPromptPath(info.token).catch(() => undefined);
    celebrate(info?.name ?? "");
  }

  function deny(): void {
    if (!info) return;
    void bridge.dismissPrompt(info.token).catch(() => undefined);
    window.close();
  }

  function celebrate(name: string): void {
    clear(host);
    const burst = el(
      "div",
      { class: "bv-prompt-burst" },
      ...Array.from({ length: 16 }, (_, i) =>
        el("i", { style: `--a:${i * 22.5}deg;--d:${120 + (i % 4) * 40}px;--t:${i * 24}ms` }),
      ),
    );
    const ring = el("div", { class: "bv-prompt-ring" }, el("i", {}), el("i", {}), el("i", {}));
    const glyph = svgIcon(ICONS.unlock, 58);
    glyph.setAttribute("stroke-width", "1.1");
    mount(
      host,
      el(
        "div",
        { class: "bv-prompt granted" },
        el("div", { class: "bv-grant-anim" }, ring, burst, el("div", { class: "bv-grant-glyph" }, glyph)),
        el("div", { class: "bv-grant-word" }, "RELEASED"),
        name ? el("div", { class: "bv-grant-name" }, name) : null,
      ),
    );
    window.setTimeout(() => window.close(), 1500);
  }

  function shortPath(p: string): string {
    const parts = p.split(/[\\/]/).filter(Boolean);
    return parts.length <= 2 ? p : `…\\${parts.slice(-2).join("\\")}`;
  }
}

function primaryBtn(label: string, onClick: () => void): HTMLButtonElement {
  const b = el("button", { class: "bv-btn primary", type: "button" }, label);
  b.addEventListener("click", onClick);
  return b;
}

function ghostBtn(label: string, onClick: () => void): HTMLButtonElement {
  const b = el("button", { class: "bv-btn ghost", type: "button" }, label);
  b.addEventListener("click", onClick);
  return b;
}
