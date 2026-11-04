// BLACKVAULT — entry point.
//
// Responsibilities, in order:
//   1. decide which window we are (primary control centre vs. secondary overlay);
//   2. boot the core state and subscribe to the event stream;
//   3. render the shell, the lock screen and (on first run) the setup wizard;
//   4. keep the lock screen and the voice read-out in sync with the core.

import "./styles/global.css";
import "./styles/lock.css";

import * as bridge from "./lib/bridge";
import { isTauri } from "./lib/bridge";
import { el, mount, toast } from "./lib/dom";
import { getState, setState, subscribe } from "./lib/store";
import { createShell, splash } from "./ui/shell";
import { createLockScreen, isOverlayWindow, renderPassiveOverlay } from "./ui/lock";
import { createWizard } from "./ui/wizard";

const root = document.getElementById("app")!;

// A secondary-monitor overlay is a very small program: black, nothing else.
if (isOverlayWindow()) {
  renderPassiveOverlay(root);
} else {
  void bootPrimary();
}

async function bootPrimary(): Promise<void> {
  const stopSplash = splash(root);
  const { toastHost } = createShell(root);
  const lock = createLockScreen(root);

  // Reflect the chosen animation variants onto the root element so CSS can read them.
  const applyAnim = (): void => {
    const s = getState().settings;
    if (!s) return;
    document.documentElement.dataset.unlockAnim = s.unlockAnimation;
    document.documentElement.dataset.lockAnim = s.lockAnimation;
  };

  // ------------------------------------------------------------- event stream
  bridge.onCoreEvent((e) => {
    switch (e.type) {
      case "status":
        setState({ status: e.payload });
        if (e.payload.mode === "locked") lock.show("locked by core");
        else if (e.payload.mode === "unlocked") lock.hide();
        break;
      case "voice":
        setState({ voice: e.payload });
        break;
      case "mic":
        setState({ mic: e.payload });
        break;
      case "mode":
        if (e.payload.mode === "locked") lock.show(e.payload.reason);
        else if (e.payload.mode === "locking") lock.show(e.payload.reason);
        else if (e.payload.mode === "unlocked") lock.hide();
        break;
      case "heard":
        setState({ heard: e.payload.text, heardFinal: e.payload.isFinal });
        lock.setHeard(e.payload.text, e.payload.isFinal);
        break;
      case "command":
        setState({ lastCommand: e.payload.phrase });
        if (e.payload.ok) toast(toastHost, `“${e.payload.phrase}”`, e.payload.action, "info");
        else toast(toastHost, `“${e.payload.phrase}”`, e.payload.action, "warn");
        break;
      case "auth":
        // A successful factor is not the end of the unlock — the second factor
        // still has to be supplied. The ceremony runs on the "unlocked" event,
        // which is the only signal that the core has actually released the
        // desktop.
        break;
      case "challenge":
        lock.setChallenge(e.payload.factors);
        break;
      case "appChallenge":
        showAppChallenge(e.payload);
        break;
      case "unlocked":
        lock.playUnlock(true, "ACCESS GRANTED", e.payload.token);
        break;
      case "event":
        setState({ events: [e.payload, ...getState().events].slice(0, 400) });
        break;
      case "settings":
        setState({ settings: e.payload });
        applyAnim();
        break;
      case "toast":
        toast(toastHost, e.payload.title, e.payload.body, e.payload.severity);
        break;
    }
  });

  subscribe(() => applyAnim());

  // A protected application was opened outside BLACKVAULT. It is still running:
  // the guard holds it for a grace period instead of killing it outright, and
  // this is where the user proves they are allowed to have it.
  let appPrompt: HTMLElement | null = null;
  function showAppChallenge(p: {
    ruleId: number;
    displayName: string;
    graceSecs: number;
  }): void {
    if (appPrompt) return;
    const card = el("div", { class: "bv-appchallenge" });
    const title = el("div", { class: "bv-appchallenge-title" }, `${p.displayName} is protected`);
    const body = el(
      "div",
      { class: "bv-appchallenge-body" },
      `Authenticate to keep it running. It closes in ${Math.max(0, Math.round(p.graceSecs))} seconds otherwise.`,
    );
    const pin = el("input", {
      class: "bv-input",
      type: "password",
      placeholder: "PIN or passphrase",
      autocomplete: "off",
    }) as HTMLInputElement;
    const actions = el("div", { class: "bv-appchallenge-actions" });
    const close = (): void => {
      card.remove();
      appPrompt = null;
    };
    const allow = async (): Promise<void> => {
      try {
        await bridge.submitPin(pin.value);
      } catch {
        /* fall through to the explicit authorise below */
      }
      try {
        await bridge.authorizeApp(p.ruleId);
        toast(toastHost, "Application released", `${p.displayName} may keep running.`);
      } catch (e) {
        toast(toastHost, "Could not release", String(e), "critical");
      }
      close();
    };
    const deny = (): void => {
      void bridge.setAppLock(p.ruleId, true);
      close();
    };
    pin.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") void allow();
      if (ev.key === "Escape") deny();
    });
    const allowBtn = el("button", { class: "bv-btn primary", type: "button" }, "Authenticate");
    allowBtn.addEventListener("click", () => void allow());
    const denyBtn = el("button", { class: "bv-btn ghost", type: "button" }, "Close it");
    denyBtn.addEventListener("click", deny);
    actions.append(allowBtn, denyBtn);
    card.append(title, body, pin, actions);
    root.appendChild(card);
    appPrompt = card;
    pin.focus();
  }

  // ------------------------------------------------------------------- boot
  try {
    const [status, settings, voice, mic, hello, commands, events] = await Promise.all([
      bridge.getStatus(),
      bridge.getSettings(),
      bridge.getVoiceStatus(),
      bridge.getMicStatus(),
      bridge.getHelloStatus(),
      bridge.listCommands(),
      bridge.listEvents(200),
    ]);
    setState({ status, settings, voice, mic, hello, commands, events, booted: true });
    applyAnim();

    if (status.mode === "locked") lock.show("session restored");

    stopSplash();

    // First run: the setup wizard. Skippable, but shown once.
    if (!settings.setupComplete) {
      setTimeout(() => {
        createWizard(root, () => {
          toast(toastHost, "Setup complete", "BLACKVAULT is armed.");
        });
      }, 400);
    }

    // Keep the microphone level and the lock-screen waveform alive.
    startLevelPolling(lock);
  } catch (err) {
    stopSplash();
    renderCoreError(root, String(err));
  }

  // In a plain browser (no Tauri) this page still renders so the UI can be reviewed.
  if (!isTauri()) {
    toast(toastHost, "Preview mode", "Running without the native core — actions are disabled.");
  }
}

/** Poll the core for the live input level so the waveform breathes. */
function startLevelPolling(lock: ReturnType<typeof createLockScreen>): void {
  let busy = false;
  setInterval(async () => {
    if (busy) return;
    busy = true;
    try {
      const mic = await bridge.getMicStatus();
      const prev = getState().mic;
      if (!prev || prev.level !== mic.level || prev.muted !== mic.muted) {
        setState({ mic });
        lock.setLevel(mic.level);
      }
    } catch {
      /* core unavailable */
    } finally {
      busy = false;
    }
  }, 350);
}

function renderCoreError(host: HTMLElement, message: string): void {
  mount(
    host,
    el("div", { class: "bv-ambient" }),
    el(
      "div",
      {
        style:
          "position:fixed;inset:0;display:flex;align-items:center;justify-content:center;flex-direction:column;gap:16px;text-align:center;padding:40px;z-index:50",
      },
      el("div", { class: "bv-brand-name", style: "letter-spacing:0.4em" }, "BLACKVAULT"),
      el("div", { class: "bv-title", style: "font-size:20px" }, "The security core did not start"),
      el(
        "div",
        { class: "bv-hint", style: "max-width:520px" },
        "The native core is required. If you are viewing this in a browser, launch the BLACKVAULT desktop application instead.",
      ),
      el("div", { class: "bv-code", style: "max-width:640px" }, message),
    ),
  );
}
