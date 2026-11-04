// BLACKVAULT — File & Folder Protection.
//
// Protection is real: NTFS DACL tightening (deny for the interactive user) plus
// EFS encryption for content. The ACL is relaxed only while the user is
// authenticated, and re-applied on lock. Hiding or renaming is never used.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon, relTime, fmtBytes } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { paneHead, type Pane, type PaneCtx } from "../ui/pane";
import type { ProtectedItem } from "../lib/types";

export const filesPane: Pane = {
  id: "files",
  title: "Files",
  render(host, ctx) {
    const listEl = el("div", { class: "bv-list" });

    const addFile = el("button", { class: "bv-btn", type: "button" });
    addFile.appendChild(svgIcon(ICONS.file, 15));
    addFile.appendChild(document.createTextNode("Protect File"));
    addFile.addEventListener("click", () => pick(ctx, "file", reload));

    const addFolder = el("button", { class: "bv-btn primary", type: "button" });
    addFolder.appendChild(svgIcon(ICONS.folder, 15));
    addFolder.appendChild(document.createTextNode("Protect Folder"));
    addFolder.addEventListener("click", () => pick(ctx, "folder", reload));

    mount(
      host,
      paneHead("File & Folder Protection", "NTFS access control + EFS content encryption", addFile, addFolder),
      el(
        "div",
        { class: "bv-card", style: "margin-bottom:16px" },
        el("div", { class: "bv-card-title" }, svgIcon(ICONS.info, 16), "How protection works"),
        el(
          "div",
          { class: "bv-hint" },
          "A protected item is not hidden or renamed. Its NTFS permissions are rewritten so the interactive user is denied, the content is encrypted with EFS, and the item is released only after you authenticate. If EFS is unavailable on the volume, BLACKVAULT falls back to DPAPI-sealed storage and says so in the dashboard.",
        ),
      ),
      listEl,
    );

    async function reload(): Promise<void> {
      try {
        const items = await bridge.listProtectedItems();
        render(listEl, items, ctx, reload);
      } catch (e) {
        mount(listEl, el("div", { class: "bv-empty" }, String(e)));
      }
    }
    void reload();
  },
};

function pick(ctx: PaneCtx, kind: "file" | "folder", reload: () => Promise<void>): void {
  bridge
    .pickPath(kind)
    .then(async (path) => {
      if (!path) return;
      try {
        const item = await bridge.protectPath(path, "acl+efs");
        ctx.toast("Protected", `${item.displayName} · ${item.encryption.toUpperCase()}`);
        void reload();
      } catch (e) {
        ctx.toast("Protection failed", String(e), "critical");
      }
    })
    .catch((e) => ctx.toast("Picker failed", String(e), "critical"));
}

function render(listEl: HTMLElement, items: ProtectedItem[], ctx: PaneCtx, reload: () => Promise<void>): void {
  if (!items.length) {
    mount(listEl, el("div", { class: "bv-empty" }, "No protected files or folders yet."));
    return;
  }
  mount(
    listEl,
    ...items.map((it) =>
      el(
        "div",
        { class: "bv-row" },
        el("span", { class: `bv-badge ${it.kind === "folder" ? "warn" : "ok"}` }, it.kind === "folder" ? "DIR" : "FILE"),
        el(
          "div",
          { class: "bv-row-main" },
          el("div", { class: "bv-row-name" }, it.displayName),
          el(
            "div",
            { class: "bv-row-meta" },
            `${it.path} · ${it.encryption.toUpperCase()} · ${fmtBytes(it.sizeBytes)}${it.lastAccess ? ` · accessed ${relTime(it.lastAccess)}` : ""}`,
          ),
        ),
        el(
          "div",
          { class: "bv-row-actions" },
          el("span", { class: `bv-badge ${it.protected ? "danger" : "ok"}` }, it.protected ? "Sealed" : "Open"),
          btn("Open", "ghost sm", async () => {
            try {
              await bridge.revealPath(it.id);
              ctx.toast("Authentication requested", `${it.displayName} · answer the BLACKVAULT card`);
            } catch (e) {
              ctx.toast("Could not request access", String(e), "critical");
            }
          }),
          btn("Unprotect", "danger sm", async () => {
            try {
              await bridge.unprotectPath(it.id);
              ctx.toast("Unprotected", it.displayName);
              void reload();
            } catch (e) {
              ctx.toast("Failed", String(e), "critical");
            }
          }),
        ),
      ),
    ),
  );
}

function btn(text: string, variant: string, onClick: () => void | Promise<void>): HTMLButtonElement {
  const b = el("button", { class: `bv-btn${variant && variant !== "default" ? " " + variant : ""}`, type: "button" });
  b.appendChild(document.createTextNode(text));
  b.addEventListener("click", () => void onClick());
  return b;
}
