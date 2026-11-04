// BLACKVAULT — pane contract. Each pane renders itself into a host element.

export interface PaneCtx {
  toast: (title: string, body?: string, severity?: string) => void;
  host: HTMLElement;
}

export interface Pane {
  readonly id: string;
  readonly title: string;
  render(host: HTMLElement, ctx: PaneCtx): void;
}

/** Standard pane header used by every route. */
export function paneHead(
  title: string,
  subtitle: string,
  ...actions: (Node | null)[]
): HTMLElement {
  const head = document.createElement("div");
  head.className = "bv-pane-head";
  const left = document.createElement("div");
  const h = document.createElement("h1");
  h.className = "bv-title";
  h.textContent = title;
  const s = document.createElement("p");
  s.className = "bv-subtitle";
  s.textContent = subtitle;
  left.appendChild(h);
  left.appendChild(s);
  head.appendChild(left);
  const right = document.createElement("div");
  right.className = "bv-toolbar";
  for (const a of actions) if (a) right.appendChild(a);
  head.appendChild(right);
  return head;
}
