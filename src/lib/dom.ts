// BLACKVAULT — minimal DOM builder. No framework, no virtual DOM, no innerHTML
// for user data (avoids injection). Text always goes through textContent.

type Child = Node | string | number | null | undefined | false | Child[];

export interface Attrs {
  class?: string;
  id?: string;
  title?: string;
  type?: string;
  value?: string | number;
  placeholder?: string;
  min?: string | number;
  max?: string | number;
  step?: string | number;
  inputmode?: string;
  maxlength?: string | number;
  minlength?: string | number;
  autocomplete?: string;
  name?: string;
  rows?: string | number;
  disabled?: boolean;
  checked?: boolean;
  selected?: boolean;
  autofocus?: boolean;
  spellcheck?: boolean;
  role?: string;
  tabindex?: number;
  "aria-label"?: string;
  style?: string;
  href?: string;
  target?: string;
  rel?: string;
  [dataAttr: `data-${string}`]: string | number | boolean | undefined;
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs?: Attrs | null,
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === undefined || v === null || v === false) continue;
      if (k === "class") node.className = String(v);
      else if (k === "value") (node as HTMLInputElement).value = String(v);
      else if (k === "checked") (node as HTMLInputElement).checked = Boolean(v);
      else if (k === "selected") (node as HTMLOptionElement).selected = Boolean(v);
      else if (k === "disabled") (node as HTMLButtonElement).disabled = Boolean(v);
      else if (k.startsWith("data-") || k.startsWith("aria-")) node.setAttribute(k, String(v));
      else node.setAttribute(k, String(v));
    }
  }
  append(node, children);
  return node;
}

export function append(parent: Node, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || typeof c === "boolean") continue;
    if (Array.isArray(c)) append(parent, c);
    else if (c instanceof Node) parent.appendChild(c);
    else parent.appendChild(document.createTextNode(String(c)));
  }
}

export function frag(...children: Child[]): DocumentFragment {
  const f = document.createDocumentFragment();
  append(f, children);
  return f;
}

export function clear(node: Element): void {
  while (node.firstChild) node.removeChild(node.firstChild);
}

/** Replace the entire contents of `node` with `children`. */
export function mount(node: Element, ...children: Child[]): void {
  clear(node);
  append(node, children);
}

export function svgIcon(path: string | undefined, size = 16, extra?: string): SVGSVGElement {
  if (!path) path = "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17Zm0 4h.01M12 11v6";
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.6");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  if (extra) svg.setAttribute("class", extra);
  const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
  p.setAttribute("d", path);
  svg.appendChild(p);
  return svg;
}

export function button(
  label: string,
  opts: { variant?: "default" | "primary" | "danger" | "ghost"; icon?: SVGSVGElement; onClick?: () => void; small?: boolean; disabled?: boolean; title?: string } = {},
): HTMLButtonElement {
  const b = el("button", {
    class: `bv-btn${opts.variant && opts.variant !== "default" ? " " + opts.variant : ""}${opts.small ? " sm" : ""}`,
    type: "button",
    title: opts.title,
    disabled: opts.disabled,
  });
  if (opts.icon) b.appendChild(opts.icon);
  b.appendChild(document.createTextNode(label));
  if (opts.onClick) b.addEventListener("click", opts.onClick);
  return b;
}

export function field(label: string, control: HTMLElement, hint?: string): HTMLElement {
  return el(
    "div",
    { class: "bv-field" },
    el("label", { class: "bv-label" }, label),
    control,
    hint ? el("div", { class: "bv-hint" }, hint) : null,
  );
}

export function switchControl(on: boolean, onChange: (v: boolean) => void): HTMLElement {
  const s = el("div", { class: `bv-switch${on ? " on" : ""}`, role: "switch", tabindex: 0 });
  const toggle = () => {
    on = !on;
    s.classList.toggle("on", on);
    onChange(on);
  };
  s.addEventListener("click", toggle);
  s.addEventListener("keydown", (e) => {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      toggle();
    }
  });
  return s;
}

export function toast(root: HTMLElement, title: string, body = "", severity = "info"): void {
  const node = el(
    "div",
    { class: "bv-toast", "data-sev": severity },
    el("div", {}, el("div", { class: "bv-toast-title" }, title), body ? el("div", { class: "bv-toast-body" }, body) : null),
  );
  root.appendChild(node);
  setTimeout(() => {
    node.classList.add("leaving");
    setTimeout(() => node.remove(), 300);
  }, 3800);
}

export function relTime(iso: string | null): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const s = Math.max(0, Math.floor((Date.now() - t) / 1000));
  if (s < 5) return "just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d}d ago`;
  return new Date(t).toLocaleDateString();
}

export function fmtBytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

export function debounce<T extends (...a: never[]) => void>(fn: T, ms: number): T {
  let h: ReturnType<typeof setTimeout> | null = null;
  return ((...args: never[]) => {
    if (h) clearTimeout(h);
    h = setTimeout(() => fn(...args), ms);
  }) as T;
}
