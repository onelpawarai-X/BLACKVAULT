// BLACKVAULT — Security event log. The audit trail. Read-only, exportable.

import * as bridge from "../lib/bridge";
import { el, mount, svgIcon } from "../lib/dom";
import { ICONS } from "../lib/icons";
import { getState, setState } from "../lib/store";
import { paneHead, type Pane } from "../ui/pane";
import type { SecurityEvent } from "../lib/types";

const FILTERS: { id: string; label: string }[] = [
  { id: "all", label: "All" },
  { id: "critical", label: "Critical" },
  { id: "warn", label: "Warning" },
  { id: "info", label: "Info" },
];

export const eventsPane: Pane = {
  id: "events",
  title: "Events",
  render(host, ctx) {
    let filter = "all";
    let query = "";
    const listEl = el("div", { class: "bv-list" });

    const search = el("input", { class: "bv-input", placeholder: "Search events…", style: "max-width:280px" });
    search.addEventListener("input", () => { query = search.value.toLowerCase(); draw(); });

    const filters = el("div", { class: "bv-toolbar" });
    for (const f of FILTERS) {
      const b = el("button", { class: `bv-tab${f.id === "all" ? " is-active" : ""}`, type: "button" });
      b.appendChild(document.createTextNode(f.label));
      b.addEventListener("click", () => {
        filter = f.id;
        for (const c of Array.from(filters.children)) c.classList.toggle("is-active", c === b);
        draw();
      });
      filters.appendChild(b);
    }

    const exportBtn = el("button", { class: "bv-btn", type: "button" });
    exportBtn.appendChild(svgIcon(ICONS.download, 15));
    exportBtn.appendChild(document.createTextNode("Export Audit"));
    exportBtn.addEventListener("click", async () => {
      try {
        const path = await bridge.exportAudit();
        ctx.toast("Audit exported", path);
      } catch (e) {
        ctx.toast("Export failed", String(e), "critical");
      }
    });

    const clearBtn = el("button", { class: "bv-btn danger", type: "button" });
    clearBtn.appendChild(svgIcon(ICONS.trash, 15));
    clearBtn.appendChild(document.createTextNode("Clear Log"));
    clearBtn.addEventListener("click", async () => {
      await bridge.clearEvents();
      setState({ events: [] });
      ctx.toast("Audit log cleared");
      void reload();
    });

    mount(
      host,
      paneHead("Security Events", "Append-only audit trail", search, filters, exportBtn, clearBtn),
      listEl,
    );

    function draw(): void {
      const events = getState().events.filter(
        (e) =>
          (filter === "all" || e.severity === filter) &&
          (!query || e.message.toLowerCase().includes(query) || e.kind.includes(query) || (e.detail ?? "").toLowerCase().includes(query)),
      );
      renderList(listEl, events);
    }

    async function reload(): Promise<void> {
      try {
        const events = await bridge.listEvents(400);
        setState({ events });
        draw();
      } catch (e) {
        mount(listEl, el("div", { class: "bv-empty" }, String(e)));
      }
    }
    void reload();
  },
};

function renderList(listEl: HTMLElement, events: SecurityEvent[]): void {
  if (!events.length) {
    mount(listEl, el("div", { class: "bv-empty" }, "No matching events."));
    return;
  }
  mount(
    listEl,
    ...events.map((e) =>
      el(
        "div",
        { class: "bv-row" },
        el("span", { class: `bv-dot ${e.severity === "critical" ? "off" : e.severity === "warn" ? "warn" : "on"}` }),
        el(
          "div",
          { class: "bv-row-main" },
          el("div", { class: "bv-row-name" }, e.message),
          el("div", { class: "bv-row-meta" }, `${e.kind}${e.detail ? ` · ${e.detail}` : ""}`),
        ),
        el("div", { class: "bv-row-meta" }, new Date(e.at).toLocaleString()),
      ),
    ),
  );
}
