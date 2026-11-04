// BLACKVAULT — Voice Application Launcher. Scan, pick, name, and give each app
// its own spoken phrase. Only allow-listed apps can ever be launched by voice.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, relTime } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { paneHead, type Pane, type PaneCtx } from "../ui/pane";
import type { LauncherApp } from "../lib/types";

export const launcherPane: Pane = {
  id: "launcher",
  title: "Launcher",
  render(host, ctx) {
    const listEl = el("div", { class: "bv-list" });

    const scanBtn = el("button", { class: "bv-btn", type: "button" });
    scanBtn.appendChild(svgIcon(ICONS.search, 15));
    scanBtn.appendChild(document.createTextNode("Scan Installed Apps"));
    scanBtn.addEventListener("click", () => openScanner(ctx, reload));

    const addBtn = el("button", { class: "bv-btn primary", type: "button" });
    addBtn.appendChild(svgIcon(ICONS.plus, 15));
    addBtn.appendChild(document.createTextNode("Add Application"));
    addBtn.addEventListener("click", () => openAddApp(ctx, reload));

    mount(
      host,
      paneHead("Voice Launcher", "Speak to open · only approved apps can launch", scanBtn, addBtn),
      listEl,
    );

    async function reload(): Promise<void> {
      try {
        const apps = await bridge.listApps();
        renderList(listEl, apps, ctx, reload);
      } catch (e) {
        mount(listEl, el("div", { class: "bv-empty" }, String(e)));
      }
    }
    void reload();
  },
};

function renderList(listEl: HTMLElement, apps: LauncherApp[], ctx: PaneCtx, reload: () => Promise<void>): void {
  if (!apps.length) {
    mount(
      listEl,
      el(
        "div",
        { class: "bv-empty" },
        "No applications configured. Add one, then say its phrase — for example “Open Chrome”.",
      ),
    );
    return;
  }
  mount(
    listEl,
    ...apps.map((a) =>
      el(
        "div",
        { class: "bv-row" },
        el("span", { class: `bv-dot ${a.enabled ? "on" : "off"}` }),
        el(
          "div",
          { class: "bv-row-main" },
          el("div", { class: "bv-row-name" }, a.name),
          el(
            "div",
            { class: "bv-row-meta" },
            `${a.voicePhrase ? `“${a.voicePhrase}”` : "no phrase"} · ${a.command} · launched ${a.launchCount}×${a.lastLaunched ? ` · ${relTime(a.lastLaunched)}` : ""}`,
          ),
        ),
        el(
          "div",
          { class: "bv-row-actions" },
          btn("Launch", "ghost sm", async () => {
            try {
              await bridge.launchApp(a.id);
              ctx.toast("Launched", a.name);
              void reload();
            } catch (e) {
              ctx.toast("Launch failed", String(e), "critical");
            }
          }),
          btn("Close", "ghost sm", async () => {
            try {
              await bridge.closeApp(a.id);
              ctx.toast("Close signal sent", a.name);
            } catch (e) {
              ctx.toast("Close failed", String(e), "warn");
            }
          }),
          btn("Edit", "ghost sm", () => openEditApp(a, ctx, reload)),
          btn("Remove", "danger sm", async () => {
            await bridge.deleteApp(a.id);
            ctx.toast("Removed", a.name);
            void reload();
          }),
        ),
      ),
    ),
  );
}

function openAddApp(ctx: PaneCtx, reload: () => Promise<void>): void {
  void bridge
    .pickExecutable()
    .then((path) => {
      if (!path) return;
      const name = path.split(/[\\/]/).pop()!.replace(/\.exe$/i, "");
      const draft: Partial<LauncherApp> = { name, command: path, voicePhrase: `open ${name.toLowerCase()}`, enabled: true };
      openEditApp(draft, ctx, reload);
    })
    .catch((e) => ctx.toast("Could not open file picker", String(e), "critical"));
}

function openEditApp(app: Partial<LauncherApp>, ctx: PaneCtx, reload: () => Promise<void>): void {
  const name = el("input", { class: "bv-input", placeholder: "Display name", value: app.name ?? "" });
  const command = el("input", { class: "bv-input", placeholder: "C:\\Path\\to\\app.exe", value: app.command ?? "" });
  const args = el("input", { class: "bv-input", placeholder: "Arguments (optional)", value: app.args ?? "" });
  const phrase = el("input", { class: "bv-input", placeholder: 'e.g. "open chrome"', value: app.voicePhrase ?? "" });

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
        el("h2", { class: "bv-modal-title" }, app.id ? "Edit Application" : "Add Application"),
        el("div", { class: "bv-modal-desc" }, "The voice phrase is matched locally. Nothing is launched that is not listed here."),
      ),
      iconBtn(ICONS.close, close),
    ),
    label("Display name", name),
    label("Executable", command),
    label("Arguments", args),
    label("Voice phrase", phrase),
    el(
      "div",
      { style: "display:flex;gap:10px;justify-content:flex-end;margin-top:10px" },
      btn("Cancel", "ghost", close),
      btn("Save", "primary", async () => {
        if (!name.value.trim() || !command.value.trim()) {
          ctx.toast("Name and executable are required", "", "warn");
          return;
        }
        try {
          await bridge.upsertApp({
            id: app.id,
            name: name.value.trim(),
            command: command.value.trim(),
            args: args.value.trim() || null,
            cwd: app.cwd ?? null,
            voicePhrase: phrase.value.trim() || null,
            enabled: app.enabled ?? true,
          });
          close();
          ctx.toast("Application saved", name.value.trim());
          void reload();
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

function openScanner(ctx: PaneCtx, reload: () => Promise<void>): void {
  const scrim = el("div", { class: "bv-scrim" });
  const body = el("div", { style: "max-height:52vh;overflow:auto" }, el("div", { class: "bv-empty" }, "Scanning the Start Menu and Program Files…"));
  const modal = el(
    "div",
    { class: "bv-modal", style: "width:min(720px,94vw)" },
    el(
      "div",
      { class: "bv-modal-head" },
      el(
        "div",
        {},
        el("h2", { class: "bv-modal-title" }, "Installed Applications"),
        el("div", { class: "bv-modal-desc" }, "Select an application to add it to the launcher."),
      ),
      iconBtn(ICONS.close, () => scrim.remove()),
    ),
    body,
  );
  scrim.appendChild(modal);
  scrim.addEventListener("click", (e) => {
    if (e.target === scrim) scrim.remove();
  });
  document.body.appendChild(scrim);

  bridge
    .scanInstalledApps()
    .then((results) => {
      if (!results.length) {
        mount(body, el("div", { class: "bv-empty" }, "No applications found."));
        return;
      }
      mount(
        body,
        el(
          "div",
          { class: "bv-list" },
          ...results.map((r) =>
            el(
              "div",
              { class: "bv-row" },
              el("div", { class: "bv-row-main" }, el("div", { class: "bv-row-name" }, r.name), el("div", { class: "bv-row-meta" }, r.exePath)),
              el(
                "div",
                { class: "bv-row-actions" },
                btn("Add", "ghost sm", async () => {
                  try {
                    await bridge.upsertApp({
                      name: r.name,
                      command: r.exePath,
                      voicePhrase: `open ${r.name.toLowerCase()}`,
                      enabled: true,
                    });
                    ctx.toast("Added", r.name);
                    void reload();
                  } catch (e) {
                    ctx.toast("Add failed", String(e), "critical");
                  }
                }),
              ),
            ),
          ),
        ),
      );
    })
    .catch((e) => mount(body, el("div", { class: "bv-empty" }, String(e))));
}

function label(text: string, control: HTMLElement): HTMLElement {
  return el("div", { style: "margin-bottom:14px" }, el("label", { class: "bv-label" }, text), control);
}

function iconBtn(icon: string, onClick: () => void): HTMLButtonElement {
  const b = el("button", { class: "bv-btn icon ghost", type: "button" }, svgIcon(icon, 16));
  b.addEventListener("click", onClick);
  return b;
}

function btn(text: string, variant: string, onClick: () => void | Promise<void>): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant && variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(text));
  b.addEventListener("click", () => void onClick());
  return b;
}
