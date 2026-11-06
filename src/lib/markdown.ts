// BLACKVAULT — a small, safe Markdown renderer for agent answers.
//
// The agent writes Markdown: headings, lists, tables, fenced code and links.
// Showing that as raw text was the complaint — the answer looked like a wall of
// asterisks and backticks, and links were not clickable.
//
// This builds real DOM nodes and never touches `innerHTML`, so nothing the model
// writes can inject markup or script. Only `http`, `https` and `mailto` links are
// turned into anchors, and every link is opened through the system browser by
// the caller rather than inside the webview.

import { el } from "./dom";

/** Inline tokens, in precedence order. */
const INLINE =
  /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(__[^_\n]+__)|(\*[^*\n]+\*)|(_[^_\n]+_)|(~~[^~\n]+~~)|(\[[^\]\n]+\]\([^)\s]+\))|(<?https?:\/\/[^\s<>()]+>?)/g;

/** Whether a URL is safe to make clickable. */
export function safeUrl(url: string): string | null {
  const trimmed = url.trim().replace(/^<|>$/g, "");
  if (/^https?:\/\//i.test(trimmed) || /^mailto:/i.test(trimmed)) return trimmed;
  return null;
}

/** Build the inline nodes for one line of text. */
function inline(text: string, onLink: (url: string) => void): Node[] {
  const out: Node[] = [];
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    const start = m.index ?? 0;
    if (start > last) out.push(document.createTextNode(text.slice(last, start)));
    const token = m[0];
    if (token.startsWith("`")) {
      out.push(el("code", { class: "bv-md-code" }, token.slice(1, -1)));
    } else if (token.startsWith("**") || token.startsWith("__")) {
      out.push(el("strong", {}, token.slice(2, -2)));
    } else if (token.startsWith("~~")) {
      out.push(el("s", {}, token.slice(2, -2)));
    } else if (token.startsWith("*") || token.startsWith("_")) {
      out.push(el("em", {}, token.slice(1, -1)));
    } else if (token.startsWith("[")) {
      const close = token.indexOf("](");
      const label = token.slice(1, close);
      const url = safeUrl(token.slice(close + 2, -1));
      if (url) out.push(link(label, url, onLink));
      else out.push(document.createTextNode(token));
    } else {
      // A bare URL. Trailing sentence punctuation is not part of it.
      const url = safeUrl(token.replace(/[.,;:!?]+$/, ""));
      if (url) {
        out.push(link(url, url, onLink));
        const tail = token.slice(token.replace(/[.,;:!?]+$/, "").length);
        if (tail) out.push(document.createTextNode(tail));
      } else {
        out.push(document.createTextNode(token));
      }
    }
    last = start + token.length;
  }
  if (last < text.length) out.push(document.createTextNode(text.slice(last)));
  return out;
}

function link(label: string, url: string, onLink: (url: string) => void): HTMLAnchorElement {
  const a = el("a", { class: "bv-md-link", href: url, rel: "noreferrer noopener", title: url }, label);
  a.addEventListener("click", (e) => {
    e.preventDefault();
    onLink(url);
  });
  return a;
}

interface ListFrame {
  list: HTMLElement;
  indent: number;
}

/**
 * Render Markdown into a fragment of real nodes.
 *
 * `onLink` is called with a safe URL when a link is clicked; the pane opens it in
 * the system browser.
 */
export function renderMarkdown(text: string, onLink: (url: string) => void): DocumentFragment {
  const frag = document.createDocumentFragment();
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const lists: ListFrame[] = [];
  let paragraph: string[] = [];
  let i = 0;

  const flushParagraph = (): void => {
    if (!paragraph.length) return;
    const p = el("p", { class: "bv-md-p" });
    paragraph.forEach((line, n) => {
      if (n) p.appendChild(el("br", {}));
      for (const node of inline(line, onLink)) p.appendChild(node);
    });
    frag.appendChild(p);
    paragraph = [];
  };

  const closeLists = (toIndent = -1): void => {
    while (lists.length && lists[lists.length - 1]!.indent > toIndent) {
      lists.pop();
    }
  };

  while (i < lines.length) {
    const line = lines[i]!;

    // Fenced code block.
    const fence = line.match(/^\s*(```+|~~~+)\s*(\S*)\s*$/);
    if (fence) {
      flushParagraph();
      closeLists();
      const marker = fence[1]!;
      const lang = fence[2] ?? "";
      const body: string[] = [];
      i++;
      while (i < lines.length && !new RegExp(`^\\s*${marker[0]}{3,}\\s*$`).test(lines[i]!)) {
        body.push(lines[i]!);
        i++;
      }
      i++; // the closing fence
      const pre = el("pre", { class: "bv-md-pre" });
      const code = el("code", { class: "bv-md-block" }, body.join("\n"));
      if (lang) pre.dataset.lang = lang;
      pre.appendChild(code);
      frag.appendChild(pre);
      continue;
    }

    if (!line.trim()) {
      flushParagraph();
      closeLists();
      i++;
      continue;
    }

    // Horizontal rule.
    if (/^\s*([-*_])\1{2,}\s*$/.test(line)) {
      flushParagraph();
      closeLists();
      frag.appendChild(el("hr", { class: "bv-md-hr" }));
      i++;
      continue;
    }

    // Heading.
    const heading = line.match(/^\s{0,3}(#{1,6})\s+(.*)$/);
    if (heading) {
      flushParagraph();
      closeLists();
      const level = Math.min(heading[1]!.length, 4);
      const h = el(`h${level + 2}` as "h3", { class: "bv-md-h" });
      for (const node of inline(heading[2]!, onLink)) h.appendChild(node);
      frag.appendChild(h);
      i++;
      continue;
    }

    // Blockquote.
    if (/^\s*>\s?/.test(line)) {
      flushParagraph();
      closeLists();
      const quoted: string[] = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i]!)) {
        quoted.push(lines[i]!.replace(/^\s*>\s?/, ""));
        i++;
      }
      const quote = el("blockquote", { class: "bv-md-quote" });
      for (const node of inline(quoted.join("\n").replace(/\n/g, " "), onLink)) {
        quote.appendChild(node);
      }
      frag.appendChild(quote);
      continue;
    }

    // List item (unordered or ordered).
    const item = line.match(/^(\s*)([-*+]|\d+[.)])\s+(.*)$/);
    if (item) {
      flushParagraph();
      const indent = Math.floor(item[1]!.replace(/\t/g, "  ").length / 2);
      const ordered = /\d/.test(item[2]!);
      closeLists(indent);
      const top = lists[lists.length - 1];
      if (!top || top.indent < indent) {
        const list = el(ordered ? "ol" : "ul", { class: "bv-md-list" });
        if (top) top.list.lastElementChild?.appendChild(list);
        else frag.appendChild(list);
        lists.push({ list, indent });
      }
      const li = el("li", {});
      for (const node of inline(item[3]!, onLink)) li.appendChild(node);
      lists[lists.length - 1]!.list.appendChild(li);
      i++;
      continue;
    }

    // Table: a header row, a separator row, then body rows.
    if (
      line.includes("|") &&
      i + 1 < lines.length &&
      /^\s*\|?[\s:|-]+\|?\s*$/.test(lines[i + 1]!) &&
      /-/.test(lines[i + 1]!)
    ) {
      flushParagraph();
      closeLists();
      const cells = (row: string): string[] =>
        row
          .trim()
          .replace(/^\||\|$/g, "")
          .split("|")
          .map((c) => c.trim());
      const table = el("table", { class: "bv-md-table" });
      const thead = el("thead", {});
      const headRow = el("tr", {});
      for (const cell of cells(line)) headRow.appendChild(el("th", {}, cell));
      thead.appendChild(headRow);
      table.appendChild(thead);
      const tbody = el("tbody", {});
      i += 2;
      while (i < lines.length && lines[i]!.includes("|") && lines[i]!.trim()) {
        const bodyRow = el("tr", {});
        for (const cell of cells(lines[i]!)) bodyRow.appendChild(el("td", {}, cell));
        tbody.appendChild(bodyRow);
        i++;
      }
      table.appendChild(tbody);
      frag.appendChild(table);
      continue;
    }

    // Anything else is part of a paragraph.
    closeLists();
    paragraph.push(line);
    i++;
  }

  flushParagraph();
  closeLists();
  return frag;
}
