/**
 * Reading a note's text as Markdown, for showing it formatted. The note itself
 * is never changed: this only describes how to show it.
 *
 * What's understood: headings (`#` and underlined), paragraphs, emphasis,
 * strong text, strikethrough (`~~`), code spans and fenced code, quotes,
 * bullet, numbered and task lists, rules, pipe tables, links and pictures.
 * HTML is never interpreted, so `<b>` stays the text "<b>". Links and
 * pictures are only described: a link's address is shown next to its text and
 * nothing is opened or loaded.
 *
 * Unlike CommonMark, a line break inside a paragraph is kept (notes are
 * written as plain text) and indented lines aren't code, so indentation typed
 * into a note shows as it was typed.
 */

export type Inline =
  | { type: "text"; text: string }
  | { type: "code"; text: string }
  | { type: "em" | "strong" | "del"; children: Inline[] }
  | { type: "link"; url: string; children: Inline[] }
  | { type: "image"; url: string; alt: string };

export type Align = "left" | "center" | "right" | null;

export type ListItem = { task: boolean | null; blocks: Block[] };

export type Block =
  | { type: "heading"; level: number; content: Inline[] }
  | { type: "paragraph"; content: Inline[] }
  | { type: "code"; info: string; text: string }
  | { type: "quote"; blocks: Block[] }
  | { type: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { type: "rule" }
  | { type: "table"; align: Align[]; head: Inline[][]; rows: Inline[][][] };

/** How deep quotes and lists may nest before the rest is shown as text. */
const MAX_DEPTH = 32;

export function parseMarkdown(text: string): Block[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const front = frontMatterEnd(lines);
  if (front === null) return parseBlocks(lines, 0);
  // Front matter (as in imported Markdown files) is shown as it is, not read
  // as a rule and an underlined heading.
  return [
    { type: "code", info: "", text: lines.slice(0, front + 1).join("\n") },
    ...parseBlocks(lines.slice(front + 1), 0),
  ];
}

/** The line closing front matter that opens the text, if there is any. */
function frontMatterEnd(lines: string[]): number | null {
  if (lines[0]?.trimEnd() !== "---") return null;
  for (let at = 1; at < lines.length; at++) {
    const line = lines[at].trimEnd();
    if (line === "---" || line === "...") return at > 1 ? at : null;
    if (line === "") return null;
  }
  return null;
}

const FENCE = /^( {0,3})(`{3,}|~{3,})(.*)$/;
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*))?$/;
const RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const UNDERLINE = /^ {0,3}(=+|-+)[ \t]*$/;
const QUOTE = /^ {0,3}>/;
const ITEM = /^( {0,3})([-*+]|(\d{1,9})([.)]))(?:([ \t]+)(.*))?$/;
const TASK = /^\[([ xX])\](?:[ \t]+|$)/;

function isBlank(line: string): boolean {
  return line.trim() === "";
}

/** A list item starting on `line`, if one does. */
function listItem(line: string) {
  const match = ITEM.exec(line);
  if (!match) return null;
  const [, indent, marker, number, delimiter, spaces = "", rest = ""] = match;
  // A marker followed by more than four spaces starts its text one space in;
  // the rest is part of the text.
  const gap = spaces.length > 4 ? 1 : Math.max(spaces.length, 1);
  return {
    ordered: number !== undefined,
    // Bullets of a different kind, or a different delimiter, start a new list.
    kind: number !== undefined ? delimiter : marker,
    start: number !== undefined ? Number(number) : 1,
    contentIndent: indent.length + marker.length + gap,
    text: spaces.length > 4 ? spaces.slice(1) + rest : rest,
    empty: rest.trim() === "",
  };
}

/** Whether `line` starts a block other than a paragraph or a table. */
function startsBlock(line: string, inParagraph: boolean): boolean {
  if (FENCE.test(line) || HEADING.test(line) || RULE.test(line) || QUOTE.test(line)) return true;
  const item = listItem(line);
  if (!item) return false;
  // As in CommonMark, only a non-empty item interrupts a paragraph, and a
  // numbered one only when it starts at 1, so "2019. A good year" carrying
  // on a paragraph stays text.
  return !inParagraph || (!item.empty && (!item.ordered || item.start === 1));
}

function parseBlocks(lines: string[], depth: number): Block[] {
  const blocks: Block[] = [];
  let paragraph: string[] = [];

  function endParagraph() {
    if (paragraph.length > 0) {
      blocks.push({ type: "paragraph", content: parseInline(paragraph.join("\n")) });
      paragraph = [];
    }
  }

  let at = 0;
  while (at < lines.length) {
    const line = lines[at];

    if (isBlank(line)) {
      endParagraph();
      at++;
      continue;
    }

    if (depth >= MAX_DEPTH) {
      paragraph.push(line);
      at++;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence && !(fence[2][0] === "`" && fence[3].includes("`"))) {
      endParagraph();
      const [, indent, marks, info] = fence;
      const closing = new RegExp(`^ {0,3}${marks[0]}{${marks.length},}[ \\t]*$`);
      // Indentation up to the opening fence's is taken off each line.
      const outdent = new RegExp(`^ {0,${indent.length}}`);
      const code: string[] = [];
      at++;
      while (at < lines.length && !closing.test(lines[at])) {
        code.push(lines[at].replace(outdent, ""));
        at++;
      }
      at++; // The closing fence, or the end of the text.
      blocks.push({ type: "code", info: info.trim(), text: code.join("\n") });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      endParagraph();
      // Closing hashes are dropped: "## Title ##" is "Title".
      const content = (heading[2] ?? "").replace(/(?:^|[ \t]+)#+[ \t]*$/, "").trim();
      blocks.push({ type: "heading", level: heading[1].length, content: parseInline(content) });
      at++;
      continue;
    }

    const underline = UNDERLINE.exec(line);
    if (underline && paragraph.length > 0) {
      const content = paragraph.join("\n").trim();
      paragraph = [];
      blocks.push({
        type: "heading",
        level: underline[1][0] === "=" ? 1 : 2,
        content: parseInline(content),
      });
      at++;
      continue;
    }

    if (RULE.test(line)) {
      endParagraph();
      blocks.push({ type: "rule" });
      at++;
      continue;
    }

    if (QUOTE.test(line)) {
      endParagraph();
      const quoted: string[] = [];
      while (at < lines.length && QUOTE.test(lines[at])) {
        quoted.push(lines[at].replace(/^ {0,3}> ?/, ""));
        at++;
      }
      blocks.push({ type: "quote", blocks: parseBlocks(quoted, depth + 1) });
      continue;
    }

    const item = listItem(line);
    if (item && startsBlock(line, paragraph.length > 0)) {
      endParagraph();
      at = parseList(lines, at, depth, blocks);
      continue;
    }

    if (paragraph.length === 0) {
      const table = parseTable(lines, at);
      if (table) {
        blocks.push(table.block);
        at = table.next;
        continue;
      }
    }

    paragraph.push(line);
    at++;
  }

  endParagraph();
  return blocks;
}

/** Reads the list starting at `lines[from]` into `blocks`; returns the next line. */
function parseList(lines: string[], from: number, depth: number, blocks: Block[]): number {
  const first = listItem(lines[from])!;
  const items: ListItem[] = [];
  let at = from;

  while (at < lines.length) {
    const item = listItem(lines[at]);
    if (!item || item.kind !== first.kind || RULE.test(lines[at])) break;

    const content: string[] = [item.text];
    at++;
    // Whether the last line taken was paragraph text, which a following line
    // can carry on without being indented.
    let carriesOn = !item.empty;
    while (at < lines.length) {
      const line = lines[at];
      if (isBlank(line)) {
        // A blank line stays in the item only if the item goes on after it.
        let next = at + 1;
        while (next < lines.length && isBlank(lines[next])) next++;
        if (next < lines.length && indentOf(lines[next]) >= item.contentIndent) {
          for (; at < next; at++) content.push("");
          carriesOn = false;
          continue;
        }
        break;
      }
      if (indentOf(line) >= item.contentIndent) {
        content.push(dedent(line, item.contentIndent));
        carriesOn = !FENCE.test(content[content.length - 1]);
        at++;
        continue;
      }
      if (carriesOn && !startsBlock(line, true) && !listItem(line)) {
        content.push(line.trimStart());
        at++;
        continue;
      }
      break;
    }

    let task: boolean | null = null;
    const taskMark = TASK.exec(content[0]);
    if (taskMark) {
      task = taskMark[1] !== " ";
      content[0] = content[0].slice(taskMark[0].length);
    }
    items.push({ task, blocks: parseBlocks(content, depth + 1) });

    if (at < lines.length && isBlank(lines[at])) {
      // Blank lines between items keep the list going.
      let next = at;
      while (next < lines.length && isBlank(lines[next])) next++;
      const following = next < lines.length ? listItem(lines[next]) : null;
      if (!following || following.kind !== first.kind) break;
      at = next;
    }
  }

  blocks.push({ type: "list", ordered: first.ordered, start: first.start, items });
  return at;
}

/** Columns of leading space, with a tab reaching the next multiple of four. */
function indentOf(line: string): number {
  let columns = 0;
  for (const char of line) {
    if (char === " ") columns++;
    else if (char === "\t") columns += 4 - (columns % 4);
    else break;
  }
  return columns;
}

/** `line` with `columns` of its leading space taken off. */
function dedent(line: string, columns: number): string {
  let taken = 0;
  let at = 0;
  while (at < line.length && taken < columns) {
    const width = line[at] === "\t" ? 4 - (taken % 4) : line[at] === " " ? 1 : 0;
    if (width === 0 || taken + width > columns) break;
    taken += width;
    at++;
  }
  return line.slice(at);
}

const DELIMITER_ROW = /^ {0,3}\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/;

/** A table whose header is `lines[at]`, if there is one there. */
function parseTable(lines: string[], at: number): { block: Block; next: number } | null {
  const header = lines[at];
  const delimiter = lines[at + 1];
  if (delimiter === undefined || !header.includes("|") || !DELIMITER_ROW.test(delimiter)) {
    return null;
  }
  const head = cells(header);
  const align: Align[] = cells(delimiter).map((cell) => {
    const left = cell.startsWith(":");
    const right = cell.endsWith(":");
    return left && right ? "center" : right ? "right" : left ? "left" : null;
  });
  if (head.length !== align.length) return null;

  const rows: Inline[][][] = [];
  let next = at + 2;
  while (next < lines.length && lines[next].includes("|") && !isBlank(lines[next])) {
    if (startsBlock(lines[next], false)) break;
    const row = cells(lines[next]);
    rows.push(align.map((_, column) => parseInline(row[column] ?? "")));
    next++;
  }
  return {
    block: { type: "table", align, head: head.map((cell) => parseInline(cell)), rows },
    next,
  };
}

/** A table row's cells, split at each `|` that isn't escaped as `\|`. */
function cells(row: string): string[] {
  let text = row.trim();
  if (text.startsWith("|")) text = text.slice(1);
  if (text.endsWith("|") && !text.endsWith("\\|")) text = text.slice(0, -1);
  const found: string[] = [];
  let cell = "";
  for (let at = 0; at < text.length; at++) {
    if (text[at] === "\\" && text[at + 1] === "|") {
      cell += "|";
      at++;
    } else if (text[at] === "|") {
      found.push(cell.trim());
      cell = "";
    } else {
      cell += text[at];
    }
  }
  found.push(cell.trim());
  return found;
}

const PUNCTUATION = /[\p{P}\p{S}]/u;
const ESCAPABLE = /[!-/:-@[-`{-~]/;
const AUTOLINK = /^<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^\s<>]*|[^\s<>@]+@[^\s<>@]+\.[^\s<>@]+)>/;

function isSpaceChar(char: string | undefined): boolean {
  if (char === undefined) return true;
  const code = char.charCodeAt(0);
  return code <= 32 ? code === 32 || (code >= 9 && code <= 13) : code > 127 && /\s/u.test(char);
}

function isPunctuation(char: string | undefined): boolean {
  return char !== undefined && PUNCTUATION.test(char);
}

/** An emphasis that has opened and not yet closed. */
type Frame = { char: string; count: number; children: Inline[] };

export function parseInline(text: string): Inline[] {
  const brackets = matchingBrackets(text);
  const ticks = backtickRuns(text);

  const root: Inline[] = [];
  const open: Frame[] = [];
  const openOf: Record<string, number> = { "*": 0, _: 0, "~": 0 };
  let plain = "";

  function out(): Inline[] {
    return open.length > 0 ? open[open.length - 1].children : root;
  }
  function pushText(more: string) {
    plain += more;
  }
  function flush() {
    if (plain !== "") {
      addText(out(), plain);
      plain = "";
    }
  }
  function push(node: Inline) {
    flush();
    out().push(node);
  }
  /** Gives up on the emphasis opened at `frame`: its marks are only text. */
  function unwind(into: Inline[], frame: Frame) {
    addText(into, frame.char.repeat(frame.count));
    for (const child of frame.children) {
      if (child.type === "text") addText(into, child.text);
      else into.push(child);
    }
  }
  function close(char: string, count: number): number {
    // The nearest emphasis opened with the same mark; those opened after it
    // can no longer close.
    let index = open.length - 1;
    while (open[index].char !== char) index--;
    const frame = open[index];
    // In order, so each piece of text is moved once.
    for (const dropped of open.splice(index + 1)) {
      openOf[dropped.char]--;
      unwind(frame.children, dropped);
    }
    const used = char === "~" ? 2 : Math.min(2, count, frame.count);
    const type = char === "~" ? "del" : used === 2 ? "strong" : "em";
    const node: Inline = { type, children: frame.children };
    frame.count -= used;
    if (frame.count > 0) {
      frame.children = [node];
    } else {
      open.pop();
      openOf[char]--;
      out().push(node);
    }
    return count - used;
  }

  let at = 0;
  while (at < text.length) {
    const char = text[at];

    if (char === "\\" && ESCAPABLE.test(text[at + 1] ?? "")) {
      pushText(text[at + 1]);
      at += 2;
      continue;
    }

    if (char === "`") {
      const run = ticks.at(at);
      const end = run.closing;
      if (end !== null) {
        let code = text.slice(at + run.length, end).replace(/\n/g, " ");
        if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ") && code.trim() !== "") {
          code = code.slice(1, -1);
        }
        push({ type: "code", text: code });
        at = end + run.length;
      } else {
        pushText("`".repeat(run.length));
        at += run.length;
      }
      continue;
    }

    if (char === "<") {
      const auto = AUTOLINK.exec(text.slice(at, at + 2048));
      if (auto) {
        push({ type: "link", url: auto[1], children: [{ type: "text", text: auto[1] }] });
        at += auto[0].length;
        continue;
      }
    }

    const image = char === "!" && text[at + 1] === "[";
    if (char === "[" || image) {
      const opening = image ? at + 1 : at;
      const closing = brackets.get(opening);
      const target = closing === undefined ? null : linkTarget(text, closing + 1);
      if (closing !== undefined && target) {
        const label = text.slice(opening + 1, closing);
        if (image) {
          push({ type: "image", url: target.url, alt: plainText(parseInline(label)) });
        } else {
          push({ type: "link", url: target.url, children: parseInline(label) });
        }
        at = target.end;
        continue;
      }
    }

    if (char === "*" || char === "_" || char === "~") {
      let end = at;
      while (text[end] === char) end++;
      let count = end - at;
      const before = text[at - 1];
      const after = text[end];
      const left =
        !isSpaceChar(after) &&
        (!isPunctuation(after) || isSpaceChar(before) || isPunctuation(before));
      const right =
        !isSpaceChar(before) &&
        (!isPunctuation(before) || isSpaceChar(after) || isPunctuation(after));
      let canOpen = left;
      let canClose = right;
      if (char === "_") {
        // An underscore inside a word, as in snake_case, is just a character.
        canOpen = left && (!right || isPunctuation(before));
        canClose = right && (!left || isPunctuation(after));
      }
      if (char === "~" && count !== 2) {
        canOpen = canClose = false;
      }

      flush();
      while (count > 0 && canClose && openOf[char] > 0) {
        count = close(char, count);
      }
      if (count > 0) {
        if (canOpen) {
          open.push({ char, count, children: [] });
          openOf[char]++;
        } else {
          pushText(char.repeat(count));
        }
      }
      at = end;
      continue;
    }

    pushText(char);
    at++;
  }

  flush();
  for (const frame of open) unwind(root, frame);
  return root;
}

function addText(into: Inline[], text: string) {
  const last = into[into.length - 1];
  if (last?.type === "text") last.text += text;
  else into.push({ type: "text", text });
}

/** The text of `nodes` without its formatting, as a picture's description. */
export function plainText(nodes: Inline[]): string {
  return nodes
    .map((node) => {
      switch (node.type) {
        case "text":
        case "code":
          return node.text;
        case "image":
          return node.alt;
        default:
          return plainText(node.children);
      }
    })
    .join("");
}

/** Where each `[` closes, skipping escaped brackets: found in one pass. */
function matchingBrackets(text: string): Map<number, number> {
  const found = new Map<number, number>();
  const opened: number[] = [];
  for (let at = 0; at < text.length; at++) {
    const char = text[at];
    if (char === "\\") at++;
    else if (char === "[") opened.push(at);
    else if (char === "]" && opened.length > 0) found.set(opened.pop()!, at);
  }
  return found;
}

/**
 * The runs of backticks in `text`, so a code span's end is found without
 * searching the rest of the text again for every run.
 */
function backtickRuns(text: string) {
  const byLength = new Map<number, number[]>();
  const lengthAt = new Map<number, number>();
  for (let at = 0; at < text.length; at++) {
    if (text[at] === "\\" && text[at + 1] === "`") {
      // An escaped backtick can't start a code span, but can end one.
      at++;
      continue;
    }
    if (text[at] !== "`") continue;
    let end = at;
    while (text[end] === "`") end++;
    lengthAt.set(at, end - at);
    const starts = byLength.get(end - at) ?? [];
    starts.push(at);
    byLength.set(end - at, starts);
    at = end - 1;
  }
  const next = new Map<number, number>();
  return {
    at(position: number): { length: number; closing: number | null } {
      const length = lengthAt.get(position);
      if (length === undefined) {
        // Inside a run that started earlier, as after an escaped backtick.
        let end = position;
        while (text[end] === "`") end++;
        return { length: end - position, closing: null };
      }
      const starts = byLength.get(length)!;
      let index = next.get(length) ?? 0;
      while (index < starts.length && starts[index] <= position) index++;
      next.set(length, index);
      return { length, closing: index < starts.length ? starts[index] : null };
    },
  };
}

/** A link's `(address "title")` starting at `text[from]`, if there is one. */
function linkTarget(text: string, from: number): { url: string; end: number } | null {
  if (text[from] !== "(") return null;
  // Looked for only this far, so a long text full of "[a](" stays quick.
  const limit = Math.min(text.length, from + 1024);
  let at = from + 1;
  const skipSpace = () => {
    while (at < limit && (text[at] === " " || text[at] === "\t" || text[at] === "\n")) at++;
  };
  skipSpace();

  let url = "";
  if (text[at] === "<") {
    const end = within(text, ">", at, limit);
    if (end === null || text.slice(at, end).includes("\n")) return null;
    url = text.slice(at + 1, end);
    at = end + 1;
  } else {
    let depth = 0;
    const start = at;
    while (at < limit && !isSpaceChar(text[at])) {
      if (text[at] === "\\") at++;
      else if (text[at] === "(") depth++;
      else if (text[at] === ")") {
        if (depth === 0) break;
        depth--;
      }
      at++;
    }
    url = text.slice(start, at).replace(/\\([!-/:-@[-`{-~])/g, "$1");
  }

  skipSpace();
  const quote = text[at];
  if (quote === '"' || quote === "'" || quote === "(") {
    const end = within(text, quote === "(" ? ")" : quote, at + 1, limit);
    if (end === null) return null;
    at = end + 1;
    skipSpace();
  }
  if (text[at] !== ")") return null;
  return { url, end: at + 1 };
}

/** Where `char` is first found in `text` from `from` up to `limit`. */
function within(text: string, char: string, from: number, limit: number): number | null {
  const found = text.slice(from, limit).indexOf(char);
  return found === -1 ? null : from + found;
}

/**
 * Whether showing `blocks` formatted would differ from the plain text: any
 * block but a paragraph, or anything in a paragraph but text.
 */
export function hasFormatting(blocks: Block[]): boolean {
  return blocks.some(
    (block) => block.type !== "paragraph" || block.content.some((node) => node.type !== "text"),
  );
}
