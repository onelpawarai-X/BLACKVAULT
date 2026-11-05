// BLACKVAULT — folding a live answer together.
//
// A streaming delta may carry the next chunk of text, or the whole text so far.
// The two are told apart by looking at what has already been shown, so the pane
// never prints a reply twice or drops a chunk. Kept here, apart from the DOM, so
// the rule can be tested on its own.

/**
 * Fold one delta chunk into the text shown so far.
 *
 * - a chunk that starts with what is shown is the whole text so far → replace;
 * - a chunk already at the end is a repeat → ignore;
 * - anything else is the next piece → append.
 */
export function mergeChunk(shown: string, chunk: string): string {
  if (chunk.startsWith(shown)) return chunk;
  if (shown.endsWith(chunk)) return shown;
  return shown + chunk;
}
