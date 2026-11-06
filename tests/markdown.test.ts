// BLACKVAULT — the Markdown renderer the agent pane uses for an answer.
//
// The complaints this guards against: an answer showed its own asterisks and
// backticks, and its links were not clickable. Both are covered here, along with
// the safety rule — only http/https/mailto ever become links.

import { beforeEach, describe, expect, it } from "vitest";

import { renderMarkdown, safeUrl } from "../src/lib/markdown";

function html(text: string, links: string[] = []): string {
  const host = document.createElement("div");
  host.appendChild(renderMarkdown(text, (url) => links.push(url)));
  return host.innerHTML;
}

describe("markdown", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("turns a bare URL into a clickable link", () => {
    const host = document.createElement("div");
    host.appendChild(renderMarkdown("see https://example.com/docs for more", () => {}));
    const a = host.querySelector("a.bv-md-link") as HTMLAnchorElement;
    expect(a).not.toBeNull();
    expect(a.textContent).toBe("https://example.com/docs");
    expect(a.getAttribute("href")).toBe("https://example.com/docs");
  });

  it("keeps sentence punctuation out of the link", () => {
    const host = document.createElement("div");
    host.appendChild(renderMarkdown("Go to https://example.com/x.", () => {}));
    expect(host.querySelector("a")!.textContent).toBe("https://example.com/x");
    expect(host.textContent).toBe("Go to https://example.com/x.");
  });

  it("calls the opener with the URL when a link is clicked", () => {
    const opened: string[] = [];
    const host = document.createElement("div");
    host.appendChild(renderMarkdown("[docs](https://example.com/a)", (u) => opened.push(u)));
    (host.querySelector("a") as HTMLAnchorElement).click();
    expect(opened).toEqual(["https://example.com/a"]);
  });

  it("renders emphasis, code and headings as elements, not raw markers", () => {
    const out = html("# Title\n\n**bold** and `code` and *italic*");
    expect(out).toContain("<h3");
    expect(out).toContain("<strong>bold</strong>");
    expect(out).toContain("<code");
    expect(out).toContain("<em>italic</em>");
    expect(out).not.toContain("**");
  });

  it("renders a fenced code block without interpreting its contents", () => {
    const out = html("```bash\nrm -rf /tmp/x\n```");
    expect(out).toContain("<pre");
    expect(out).toContain("rm -rf /tmp/x");
  });

  it("renders unordered and ordered lists", () => {
    const out = html("- one\n- two\n\n1. first\n2. second");
    expect(out).toContain("<ul");
    expect(out).toContain("<ol");
    expect(out.match(/<li>/g)?.length).toBe(4);
  });

  it("never turns a javascript: or file: URL into a link", () => {
    expect(safeUrl("javascript:alert(1)")).toBeNull();
    expect(safeUrl("file:///C:/Windows")).toBeNull();
    const out = html("[click](javascript:alert(1))");
    expect(out).not.toContain("<a");
  });

  it("renders a bare URL with no scheme as plain text", () => {
    const out = html("visit example.com now");
    expect(out).not.toContain("<a");
    expect(out).toContain("example.com");
  });
});
