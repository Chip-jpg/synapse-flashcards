import { describe, expect, it } from "vitest";
import { hasFormatting, parseInline, parseMarkdown, type Inline } from "./markdown";

const text = (value: string): Inline => ({ type: "text", text: value });

describe("blocks", () => {
  it("reads headings, with closing hashes dropped and underlined ones", () => {
    expect(parseMarkdown("# One\n## Two ##\n###### Six\n#tag")).toEqual([
      { type: "heading", level: 1, content: [text("One")] },
      { type: "heading", level: 2, content: [text("Two")] },
      { type: "heading", level: 6, content: [text("Six")] },
      { type: "paragraph", content: [text("#tag")] },
    ]);
    expect(parseMarkdown("Title\n=====\nPart\n---")).toEqual([
      { type: "heading", level: 1, content: [text("Title")] },
      { type: "heading", level: 2, content: [text("Part")] },
    ]);
  });

  it("keeps line breaks and indentation inside a paragraph", () => {
    expect(parseMarkdown("first line\n    indented\n\nnext")).toEqual([
      { type: "paragraph", content: [text("first line\n    indented")] },
      { type: "paragraph", content: [text("next")] },
    ]);
  });

  it("reads fenced code as it is, closed or not", () => {
    expect(parseMarkdown("```js\nlet *a* = 1;\n  <b>\n```\nafter")).toEqual([
      { type: "code", info: "js", text: "let *a* = 1;\n  <b>" },
      { type: "paragraph", content: [text("after")] },
    ]);
    expect(parseMarkdown("~~~\nnever closed")).toEqual([
      { type: "code", info: "", text: "never closed" },
    ]);
  });

  it("reads quotes, which can hold other blocks", () => {
    expect(parseMarkdown("> # Quoted\n> - item")).toEqual([
      {
        type: "quote",
        blocks: [
          { type: "heading", level: 1, content: [text("Quoted")] },
          {
            type: "list",
            ordered: false,
            start: 1,
            items: [{ task: null, blocks: [{ type: "paragraph", content: [text("item")] }] }],
          },
        ],
      },
    ]);
  });

  it("reads bullet, numbered, nested and task lists", () => {
    const [list] = parseMarkdown("- one\n  - inner\n- [x] done\n- [ ] to do\ncarried on");
    expect(list).toEqual({
      type: "list",
      ordered: false,
      start: 1,
      items: [
        {
          task: null,
          blocks: [
            { type: "paragraph", content: [text("one")] },
            {
              type: "list",
              ordered: false,
              start: 1,
              items: [{ task: null, blocks: [{ type: "paragraph", content: [text("inner")] }] }],
            },
          ],
        },
        { task: true, blocks: [{ type: "paragraph", content: [text("done")] }] },
        {
          task: false,
          blocks: [{ type: "paragraph", content: [text("to do\ncarried on")] }],
        },
      ],
    });

    expect(parseMarkdown("3. three\n\n4. four")).toEqual([
      {
        type: "list",
        ordered: true,
        start: 3,
        items: [
          { task: null, blocks: [{ type: "paragraph", content: [text("three")] }] },
          { task: null, blocks: [{ type: "paragraph", content: [text("four")] }] },
        ],
      },
    ]);
  });

  it("doesn't start a list in a paragraph with a number other than 1", () => {
    expect(parseMarkdown("The year was\n2019. A good year")).toEqual([
      { type: "paragraph", content: [text("The year was\n2019. A good year")] },
    ]);
  });

  it("reads rules, and front matter as it is", () => {
    expect(parseMarkdown("a\n\n***\n\n- - -")).toEqual([
      { type: "paragraph", content: [text("a")] },
      { type: "rule" },
      { type: "rule" },
    ]);
    expect(parseMarkdown("---\ntitle: Cells\n---\n# Cells")).toEqual([
      { type: "code", info: "", text: "---\ntitle: Cells\n---" },
      { type: "heading", level: 1, content: [text("Cells")] },
    ]);
  });

  it("reads pipe tables, with alignment and escaped pipes", () => {
    expect(
      parseMarkdown("| Phase | Chromosomes |\n|:--|--:|\n| Meta | a \\| b |\n| Ana |"),
    ).toEqual([
      {
        type: "table",
        align: ["left", "right"],
        head: [[text("Phase")], [text("Chromosomes")]],
        rows: [
          [[text("Meta")], [text("a | b")]],
          [[text("Ana")], []],
        ],
      },
    ]);
    // A delimiter row that doesn't match the header isn't a table.
    expect(parseMarkdown("a | b\n--|--|--")[0].type).toBe("paragraph");
  });
});

describe("inline", () => {
  it("reads emphasis, strong text and strikethrough", () => {
    expect(parseInline("*a* **b** _c_ __d__ ~~e~~ ***f***")).toEqual([
      { type: "em", children: [text("a")] },
      text(" "),
      { type: "strong", children: [text("b")] },
      text(" "),
      { type: "em", children: [text("c")] },
      text(" "),
      { type: "strong", children: [text("d")] },
      text(" "),
      { type: "del", children: [text("e")] },
      text(" "),
      { type: "em", children: [{ type: "strong", children: [text("f")] }] },
    ]);
  });

  it("leaves marks that don't open or close as text", () => {
    expect(parseInline("snake_case_name, 2 * 3 * 4, **unclosed, ~one~")).toEqual([
      text("snake_case_name, 2 * 3 * 4, **unclosed, ~one~"),
    ]);
    expect(parseInline("**a* b")).toEqual([
      text("*"),
      { type: "em", children: [text("a")] },
      text(" b"),
    ]);
  });

  it("reads code spans, whose contents aren't formatted", () => {
    expect(parseInline("run `a *b* <i>` and `` x`y ``, not `this")).toEqual([
      text("run "),
      { type: "code", text: "a *b* <i>" },
      text(" and "),
      { type: "code", text: "x`y" },
      text(", not `this"),
    ]);
  });

  it("reads backslash escapes", () => {
    expect(parseInline("\\*not em\\* \\[x](y)")).toEqual([text("*not em* [x](y)")]);
  });

  it("reads links, autolinks and pictures, keeping only their addresses", () => {
    expect(
      parseInline(
        '[the *cell*](https://example.org/a_(b) "Title") <https://x.org> ![A cell](cell.png)',
      ),
    ).toEqual([
      {
        type: "link",
        url: "https://example.org/a_(b)",
        children: [text("the "), { type: "em", children: [text("cell")] }],
      },
      text(" "),
      { type: "link", url: "https://x.org", children: [text("https://x.org")] },
      text(" "),
      { type: "image", url: "cell.png", alt: "A cell" },
    ]);
    expect(parseInline("[not a link] (x)")).toEqual([text("[not a link] (x)")]);
  });

  it("keeps HTML as text", () => {
    expect(parseInline('<b onclick="x()">bold</b> <script>')).toEqual([
      text('<b onclick="x()">bold</b> <script>'),
    ]);
  });

  it("stays quick on long texts full of marks", () => {
    const started = performance.now();
    parseMarkdown(
      "[a](".repeat(12_000) + "*a _b ".repeat(8_000) + "`".repeat(3) + " `x".repeat(8_000),
    );
    expect(performance.now() - started).toBeLessThan(2_000);
  });
});

describe("hasFormatting", () => {
  it("is false only for paragraphs of plain text", () => {
    expect(hasFormatting(parseMarkdown("Just text.\n\nAnd more, 2 * 3."))).toBe(false);
    expect(hasFormatting(parseMarkdown("Some **bold** text."))).toBe(true);
    expect(hasFormatting(parseMarkdown("- a list"))).toBe(true);
  });
});
