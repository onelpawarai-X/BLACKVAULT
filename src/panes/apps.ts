// BLACKVAULT — App Lock. Real enforcement via a Windows Job Object + ACL guard,
// not a cosmetic list. Each protected app gets an auth requirement and an
// optional auto-relock timer.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, relTime } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { paneHead, type Pane, type PaneCtx } from "../ui/pane";
import type { ProtectedApp } from "../lib/types";

export const appsPane: Pane = {
  id: "apps",
  title: "App Lock",
  render(host, ctx) {
    const listEl = el("div", { class: "bv-list" });

    const addBtn = el("button", { class: "bv-btn primary", type: "button" });
    addBtn.appendChild(svgIcon(ICONS.plus, 15));
    addBtn.appendChild(document.createTextNode("Protect an Application"));
    addBtn.addEventListener("click", () => pickAndProtect(ctx, reload));

    mount(
      host,
      paneHead("Application Lock", "Windows job-object enforcement · auth required to run", addBtn),
      listEl,
    );

    async function reload(): Promise<void> {
      try {
        const apps = await bridge.listProtectedApps();
        render(listEl, apps, ctx, reload);
      } catch (e) {
        mount(listEl, el("div", { class: "bv-empty" }, String(e)));
      }
    }
    void reload();
  },
};

function pickAndProtect(ctx: PaneCtx, reload: () => Promise<void>): void {
  bridge
    .pickExecutable()
    .then(async (path) => {
      if (!path) return;
      const name = path.split(/[\\/]/).pop()!.replace(/\.exe$/i, "");
      try {
        await bridge.protectApp(path, name);
        ctx.toast("Application protected", name);
        void reload();
      } catch (e) {
        ctx.toast("Protect failed", String(e), "critical");
      }
    })
    .catch((e) => ctx.toast("File picker failed", String(e), "critical"));
}

function render(listEl: HTMLElement, apps: ProtectedApp[], ctx: PaneCtx, reload: () => Promise<void>): void {
  if (!apps.length) {
    mount(
      listEl,
      el("div", { class: "bv-empty" }, "No protected applications. Pick an .exe to guard it behind authentication."),
    );
    return;
  }
  mount(
    listEl,
    ...apps.map((a) =>
      el(
        "div",
        { class: "bv-row" },
        el("span", { class: `bv-dot ${a.locked ? "off" : "on"}` }),
        el(
          "div",
          { class: "bv-row-main" },
          el("div", { class: "bv-row-name" }, a.displayName),
          el(
            "div",
            { class: "bv-row-meta" },
            `${a.exePath} · ${a.requireAuth ? "auth required" : "auth off"} · ${a.autoRelock ? `relock ${a.relockAfterSecs}s` : "no auto-relock"} · added ${relTime(a.createdAt)}`,
          ),
        ),
        el(
          "div",
          { class: "bv-row-actions" },
          el("span", { class: `bv-badge ${a.locked ? "danger" : "ok"}` }, a.locked ? "Locked" : "Open"),
          btn(a.locked ? "Unlock" : "Lock", "ghost sm", async () => {
            try {
              await bridge.setAppLock(a.id, !a.locked);
              void reload();
            } catch (e) {
              ctx.toast("Failed", String(e), "critical");
            }
          }),
          btn("Settings", "ghost sm", () => editApp(a, ctx, reload)),
          btn("Remove", "danger sm", async () => {
            await bridge.unprotectApp(a.id);
            ctx.toast("Removed", a.displayName);
            void reload();
          }),
        ),
      ),
    ),
  );
}

function editApp(a: ProtectedApp, ctx: PaneCtx, reload: () => Promise<void>): void {
  let requireAuth = a.requireAuth;
  let autoRelock = a.autoRelock;
  const relockSecs = el("input", {
    class: "bv-input",
    type: "number",
    min: "5",
    max: "3600",
    value: a.relockAfterSecs,
  });
  const name = el("input", { class: "bv-input", value: a.displayName });

  const authToggle = el("div", { class: `bv-switch${requireAuth ? " on" : ""}`, tabindex: 0 });
  authToggle.addEventListener("click", () => {
    requireAuth = !requireAuth;
    authToggle.classList.toggle("on", requireAuth);
  });
  const relockToggle = el("div", { class: `bv-switch${autoRelock ? " on" : ""}`, tabindex: 0 });
  relockToggle.addEventListener("click", () => {
    autoRelock = !autoRelock;
    relockToggle.classList.toggle("on", autoRelock);
  });

  const scrim = el("div", { class: "bv-scrim" });
  const modal = el(
    "div",
    { class: "bv-modal" },
    el(
      "div",
      { class: "bv-modal-head" },
      el("div", {}, el("h2", { class: "bv-modal-title" }, "Application Lock Settings"), el("div", { class: "bv-modal-desc" }, a.exePath)),
      iconBtn(() => scrim.remove()),
    ),
    label("Display name", name),
    el("div", { style: "display:flex;align-items:center;gap:12px;margin:14px 0" }, authToggle, el("span", {}, "Require authentication to launch")),
    el("div", { style: "display:flex;align-items:center;gap:12px;margin:14px 0" }, relockToggle, el("span", {}, "Auto-relock after the app closes")),
    label("Relock delay (seconds)", relockSecs),
    el(
      "div",
      { style: "display:flex;gap:10px;justify-content:flex-end" },
      btn("Cancel", "ghost", () => scrim.remove()),
      btn("Save", "primary", async () => {
        try {
          await bridge.updateProtectedApp({
            ...a,
            displayName: name.value.trim() || a.displayName,
            requireAuth,
            autoRelock,
            relockAfterSecs: Math.max(5, Number(relockSecs.value) || 30),
          });
          scrim.remove();
          ctx.toast("Saved", name.value.trim());
          void reload();
        } catch (e) {
          ctx.toast("Save failed", String(e), "critical");
        }
      }),
    ),
  );
  scrim.appendChild(modal);
  scrim.addEventListener("click", (e) => {
    if (e.target === scrim) scrim.remove();
  });
  document.body.appendChild(scrim);
}

function label(text: string, control: HTMLElement): HTMLElement {
  return el("div", { style: "margin-bottom:14px" }, el("label", { class: "bv-label" }, text), control);
}
function iconBtn(onClick: () => void): HTMLButtonElement {
  const b = el("button", { class: "bv-btn icon ghost", type: "button" }, svgIcon(ICONS.close, 16));
  b.addEventListener("click", onClick);
  return b;
}
function btn(text: string, variant: string, onClick: () => void | Promise<void>): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant && variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(text));
  b.addEventListener("click", () => void onClick());
  return b;
}
