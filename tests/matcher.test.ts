// BLACKVAULT — frontend unit tests.
//
// These mirror the Rust-side behaviour that the UI depends on: the phrase
// normalisation rules and the DOM helpers that build every screen. They run in
// jsdom with no native core, which is exactly the point — the pure logic must
// hold on its own.

import { describe, expect, it } from "vitest";
import { el, mount, fmtBytes, relTime } from "../src/lib/dom";

// ------------------------------------------------------------------ helpers

/** The same normalisation the Rust matcher applies, re-implemented for parity. */
function normalise(phrase: string): string {
  let out = "";
  let lastSpace = true;
  for (const ch of phrase) {
    if (/[a-z0-9]/i.test(ch)) {
      out += ch.toLowerCase();
      lastSpace = false;
    } else if (!lastSpace) {
      out += " ";
      lastSpace = true;
    }
  }
  return out.replace(/ $/, "");
}

function levenshtein(a: string, b: string): number {
  const A = [...a];
  const B = [...b];
  if (A.length === 0) return B.length;
  if (B.length === 0) return A.length;
  let prev = Array.from({ length: B.length + 1 }, (_, i) => i);
  let cur = new Array<number>(B.length + 1).fill(0);
  for (let i = 1; i <= A.length; i++) {
    cur[0] = i;
    for (let j = 1; j <= B.length; j++) {
      const cost = A[i - 1] === B[j - 1] ? 0 : 1;
      cur[j] = Math.min(prev[j]! + 1, cur[j - 1]! + 1, prev[j - 1]! + cost);
    }
    [prev, cur] = [cur, prev];
  }
  return prev[B.length]!;
}

// ------------------------------------------------------------------- tests

describe("phrase normalisation", () => {
  it("lower-cases and strips punctuation", () => {
    expect(normalise("Lock this PC!")).toBe("lock this pc");
  });

  it("collapses runs of whitespace", () => {
    expect(normalise("  NEXT   VIDEO  ")).toBe("next video");
  });

  it("treats hyphens as separators", () => {
    expect(normalise("volume-up")).toBe("volume up");
  });

  it("handles empty input", () => {
    expect(normalise("")).toBe("");
    expect(normalise("!!!")).toBe("");
  });
});

describe("fuzzy phrase matching", () => {
  const phrases = ["next", "scroll down", "open chrome"];

  function bestMatch(text: string): { phrase: string; score: number } | null {
    const n = normalise(text);
    if (!n) return null;
    let best: { phrase: string; score: number } | null = null;
    for (const p of phrases) {
      const pn = normalise(p);
      const score =
        pn === n
          ? 1
          : Math.max(
              0,
              1 - levenshtein(n, pn) / Math.max(n.length, pn.length, 1),
            );
      if (!best || score > best.score) best = { phrase: p, score };
    }
    return best && best.score >= 0.6 ? best : null;
  }

  it("matches exactly", () => {
    expect(bestMatch("NEXT")?.phrase).toBe("next");
  });

  it("matches with a small typo", () => {
    expect(bestMatch("scroll dowm")?.phrase).toBe("scroll down");
  });

  it("rejects unrelated text", () => {
    expect(bestMatch("a completely different sentence")).toBeNull();
  });

  it("rejects empty input", () => {
    expect(bestMatch("   ")).toBeNull();
  });
});

describe("DOM helpers", () => {
  it("builds elements with attributes and children", () => {
    const node = el("div", { class: "card", "data-role": "test" }, "hello", el("span", {}, "world"));
    expect(node.className).toBe("card");
    expect(node.getAttribute("data-role")).toBe("test");
    expect(node.textContent).toBe("helloworld");
  });

  it("never interprets text as HTML", () => {
    const node = el("div", {}, "<img src=x onerror=alert(1)>");
    expect(node.querySelector("img")).toBeNull();
    expect(node.textContent).toBe("<img src=x onerror=alert(1)>");
  });

  it("mount replaces previous content", () => {
    const host = document.createElement("div");
    mount(host, el("p", {}, "first"));
    expect(host.textContent).toBe("first");
    mount(host, el("p", {}, "second"));
    expect(host.textContent).toBe("second");
    expect(host.children.length).toBe(1);
  });

  it("skips null and false children", () => {
    const node = el("div", {}, null, false, undefined, "kept");
    expect(node.textContent).toBe("kept");
  });
});

describe("formatting", () => {
  it("formats byte sizes", () => {
    expect(fmtBytes(512)).toBe("512 B");
    expect(fmtBytes(2048)).toBe("2.0 KB");
    expect(fmtBytes(1024 * 1024 * 5)).toBe("5.0 MB");
    expect(fmtBytes(null)).toBe("—");
  });

  it("formats relative times", () => {
    expect(relTime(null)).toBe("—");
    expect(relTime(new Date().toISOString())).toMatch(/just now|s ago/);
    expect(relTime(new Date(Date.now() - 3600_000).toISOString())).toBe("1h ago");
  });
});
