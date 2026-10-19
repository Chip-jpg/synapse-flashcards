/**
 * The most characters of a note cards are suggested from at a time:
 * `SUGGESTION_NOTE_MAX_CHARS` in `src-tauri/src/suggest.rs`.
 */
export const SUGGESTION_NOTE_MAX_CHARS = 12_000;

/**
 * A part of a note: its text, and where it is in the note, counted in
 * characters (code points, as Rust counts them) from `start` up to, but not
 * including, `end`.
 */
export type NotePart = { start: number; end: number; text: string };

function isSpace(char: string): boolean {
  return /\s/u.test(char);
}

/** Ends a sentence, so a part can end after it. */
function endsSentence(char: string): boolean {
  return ".!?…".includes(char);
}

/**
 * Where a part that starts at `start` should end, at most `max` characters
 * on: after a paragraph if one ends in its second half, else after a line,
 * then a sentence, then a word, and only otherwise in the middle of a word.
 */
function breakAt(chars: string[], start: number, max: number): number {
  const limit = start + max;
  const earliest = start + Math.floor(max / 2);
  const ways: ((at: number) => boolean)[] = [
    (at) => chars[at] === "\n" && chars[at + 1] === "\n",
    (at) => chars[at] === "\n",
    (at) => isSpace(chars[at]) && endsSentence(chars[at - 1]),
    (at) => isSpace(chars[at]),
  ];
  for (const fits of ways) {
    for (let at = limit; at > earliest; at--) {
      if (fits(at)) return at;
    }
  }
  return limit;
}

/**
 * Splits a note's `body` into parts of at most `max` characters, in order,
 * ending each part where a paragraph (or failing that, a line, sentence, or
 * word) ends. Blank space between parts belongs to neither. A note that fits
 * is one part; one with no text has none.
 */
export function noteParts(body: string, max = SUGGESTION_NOTE_MAX_CHARS): NotePart[] {
  const chars = Array.from(body);
  const parts: NotePart[] = [];
  let start = 0;
  for (;;) {
    while (start < chars.length && isSpace(chars[start])) start++;
    if (start === chars.length) return parts;
    const cut = chars.length - start <= max ? chars.length : breakAt(chars, start, max);
    let end = cut;
    while (isSpace(chars[end - 1])) end--;
    parts.push({ start, end, text: chars.slice(start, end).join("") });
    start = cut;
  }
}
