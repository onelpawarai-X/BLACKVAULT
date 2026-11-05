// BLACKVAULT — the shell must repaint its pane on every state change.
//
// The bug this guards against: panes were rendered only when the route changed.
// The first render happens before the core has answered, so a pane painted its
// empty defaults and then never repainted once the real state arrived — the
// dashboard showed "Stopped", "0" and "Unknown" for a running engine.

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { createShell } from "../src/ui/shell";
import { navigate, setState } from "../src/lib/store";
import { agentPane } from "../src/panes/agent";
import type { EngineStatus, VoiceStatus } from "../src/lib/types";

function voice(running: boolean, lastCommand: string | null): VoiceStatus {
  return {
    running,
    wakePhrase: "blackvault",
    listening: running,
    lastHeard: lastCommand,
    lastCommand,
    recoveryAttempts: 0,
    model: "windows",
    backend: "windows",
  };
}

function status(mode: EngineStatus["mode"], apps: number, protectedFiles: number): EngineStatus {
  return {
    mode,
    voice: voice(true, "open chrome"),
    mic: { deviceName: "Microphone", level: 0.3, permission: "granted", muted: false },
    hello: { available: true, enrolled: true, method: "fingerprint", lastVerified: null },
    pinConfigured: true,
    patternConfigured: true,
    passphraseConfigured: true,
    serviceRunning: true,
    protectedApps: apps,
    protectedFiles,
    protectedFolders: 4,
    uptimeSecs: 7421,
    lastAuth: null,
    speechAvailable: true,
    stt: {
      enabled: true,
      running: true,
      busy: false,
      endpoint: "https://example/v1/stt",
      language: "en",
      live: true,
      recognised: 184,
      lastText: "lock this pc",
      lastError: null,
    },
  };
}

function metrics(root: HTMLElement): string[] {
  return Array.from(root.querySelectorAll(".bv-metric")).map((e) => e.textContent ?? "");
}

describe("shell", () => {
  let root: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = "";
    root = document.createElement("div");
    document.body.appendChild(root);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("paints the pane once the core state arrives", () => {
    createShell(root);

    // Before boot: the pane is mounted but has nothing to show.
    expect(metrics(root)).toContain("Stopped");

    setState({
      status: status("unlocked", 3, 12),
      voice: voice(true, "open chrome"),
      booted: true,
    });

    const shown = metrics(root);
    expect(shown).toContain("Active");
    expect(shown).toContain("3");
    expect(shown).toContain("16");
    expect(shown).not.toContain("Stopped");
  });

  it("repaints when the state changes a second time", () => {
    createShell(root);
    setState({ status: status("unlocked", 3, 12), voice: voice(true, "open chrome") });
    expect(metrics(root)).toContain("3");

    setState({ status: status("locked", 9, 40) });
    const shown = metrics(root);
    expect(shown).toContain("Locked");
    expect(shown).toContain("9");
    // Protected Files reports files + folders together.
    expect(shown).toContain("44");
  });

  it("does not rebuild a pane the user is typing into", () => {
    createShell(root);
    setState({ status: status("unlocked", 3, 12) });

    // A dashboard has no input, so stand in for one: focus inside the pane is
    // what protects a half-typed form from being wiped by an incoming event.
    const field = document.createElement("input");
    root.querySelector(".bv-pane")!.appendChild(field);
    field.focus();
    expect(root.contains(document.activeElement)).toBe(true);

    setState({ status: status("locked", 9, 40) });
    expect(root.contains(field)).toBe(true);
    expect(field.isConnected).toBe(true);
  });

  it("keeps the nav status cluster in step with the state", () => {
    createShell(root);
    setState({ status: status("locked", 0, 0), voice: voice(false, null) });
    const text = root.querySelector(".bv-nav-right")?.textContent ?? "";
    expect(text).toContain("Locked");
    expect(text).toContain("Voice Off");
  });

  it("never rebuilds the agent pane, which looks after itself", async () => {
    expect(agentPane.selfUpdating).toBe(true);

    createShell(root);
    navigate("agent");
    // The pane paints itself asynchronously from the core.
    await Promise.resolve();
    await Promise.resolve();

    const pane = root.querySelector(".bv-pane")!;
    const marker = document.createElement("i");
    marker.dataset.marker = "keep-me";
    pane.appendChild(marker);

    // A burst of state changes, as the microphone level used to cause.
    for (let i = 0; i < 12; i += 1) {
      setState({ status: status("unlocked", i, 0), voice: voice(true, "next") });
    }

    expect(pane.querySelector('[data-marker="keep-me"]')).not.toBeNull();
  });
});
