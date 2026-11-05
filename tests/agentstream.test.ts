// BLACKVAULT — the live answer must be folded together, not duplicated.
//
// A streaming delta may be the next chunk or the whole text so far. Getting this
// wrong shows the reply twice, or drops half of it.

import { describe, expect, it } from "vitest";

import { mergeChunk } from "../src/lib/agentstream";

describe("mergeChunk", () => {
  it("appends an incremental chunk", () => {
    expect(mergeChunk("Hel", "lo")).toBe("Hello");
    expect(mergeChunk("Hello", " world")).toBe("Hello world");
  });

  it("replaces when the chunk is the whole text so far", () => {
    expect(mergeChunk("Hello wor", "Hello world")).toBe("Hello world");
  });

  it("ignores a chunk already shown", () => {
    expect(mergeChunk("Hello world", "world")).toBe("Hello world");
  });

  it("builds the same text from a stream of small chunks", () => {
    let shown = "";
    for (const c of ["B", "L", "A", "C", "K"]) shown = mergeChunk(shown, c);
    expect(shown).toBe("BLACK");
  });
});
