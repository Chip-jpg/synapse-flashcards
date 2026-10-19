import { describe, expect, it } from "vitest";
import { noteParts, SUGGESTION_NOTE_MAX_CHARS } from "./noteParts";

/** Each part's text, checked to be exactly where the part says it is. */
function texts(body: string, max?: number): string[] {
  const chars = Array.from(body);
  return noteParts(body, max).map((part) => {
    expect(chars.slice(part.start, part.end).join("")).toBe(part.text);
    return part.text;
  });
}

describe("splitting a long note into parts", () => {
  it("keeps a note that fits as one part, and finds none in blank text", () => {
    expect(texts("Mitosis has four phases.")).toEqual(["Mitosis has four phases."]);
    const longest = "a".repeat(SUGGESTION_NOTE_MAX_CHARS);
    expect(texts(longest)).toEqual([longest]);
    expect(texts("")).toEqual([]);
    expect(texts(" \n\n ")).toEqual([]);
  });

  it("ends parts where paragraphs end, leaving the blank lines between them out", () => {
    const body = "Slide 1\nCells\n\nSlide 2\nMitosis\n\nSlide 3\nMeiosis";
    expect(texts(body, 30)).toEqual(["Slide 1\nCells\n\nSlide 2\nMitosis", "Slide 3\nMeiosis"]);
  });

  it("falls back to a line, then a sentence, then a word, then a hard cut", () => {
    expect(texts("one two\nthree four five", 18)).toEqual(["one two\nthree four", "five"]);
    expect(texts("Cells divide. They grow too", 20)).toEqual(["Cells divide.", "They grow too"]);
    expect(texts("alpha beta gamma delta", 12)).toEqual(["alpha beta", "gamma delta"]);
    expect(texts("abcdefghij", 4)).toEqual(["abcd", "efgh", "ij"]);
  });

  it("doesn't end a part on a break so early that it's left short", () => {
    // The paragraph break comes too soon, so the part ends at a later word.
    expect(texts("Hi\n\nthis is a longer paragraph", 20)).toEqual([
      "Hi\n\nthis is a longer",
      "paragraph",
    ]);
  });

  it("counts characters as Rust does, so accents and emoji take one each", () => {
    const body = "é🧬 é🧬 é🧬";
    const parts = noteParts(body, 6);
    expect(parts.map(({ start, end }) => [start, end])).toEqual([
      [0, 5],
      [6, 8],
    ]);
  });

  it("covers a long document in parts no longer than the limit, missing nothing", () => {
    const paragraph = "The cell cycle has phases. ".repeat(20).trim();
    const body = Array.from({ length: 60 }, (_, n) => `Page ${n + 1}\n${paragraph}`).join("\n\n");
    const parts = noteParts(body);
    expect(parts.length).toBeGreaterThan(1);
    for (const part of parts) {
      expect(part.end - part.start).toBeLessThanOrEqual(SUGGESTION_NOTE_MAX_CHARS);
      expect(part.text.startsWith("Page ")).toBe(true);
    }
    const words = (text: string) => text.split(/\s+/u).filter(Boolean);
    expect(parts.flatMap((part) => words(part.text))).toEqual(words(body));
  });
});
