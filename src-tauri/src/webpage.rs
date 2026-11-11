//! Saved web pages as note text.
//!
//! A page the user saved with the browser's "Save page as" (`.html`/`.htm`)
//! becomes Markdown, by plain string handling and nothing else: no network, no
//! script, no stylesheet is ever loaded or run, and nothing in the page is
//! opened.
//!
//! - The title is the page's `<title>`, else its `og:title`, else its first
//!   `<h1>`; the caller falls back to the file name.
//! - `script`, `style`, `noscript`, `template`, `svg`, `nav`, forms, comments
//!   and other things that are not text go. `header` and `footer` go too,
//!   unless the page has an `<article>` or `<main>` (the first `<main>`, else
//!   the first `<article>`), in which case only its content is read.
//! - Headings become `#`, lists `- ` / `1. `, quotes `> `, `<pre>` a fenced
//!   block, `<code>` a code span, and tables pipe tables. Links become
//!   `[text](address)`: the address is text, never followed. Pictures are left
//!   out and counted.
//! - Entities are decoded and white space collapsed as a browser would.

use crate::anki::{parse_entity, parse_tag};

/// Elements whose content is not text; the tokenizer drops it unread, so a `<`
/// in a script can't be taken for a tag.
const RAW_TEXT: &[&str] = &["script", "style", "textarea"];
/// Elements dropped with everything in them.
const ALWAYS_SKIPPED: &[&str] = &[
    "head", "title", "script", "style", "noscript", "template", "svg", "nav", "iframe", "object",
    "embed", "audio", "video", "canvas", "select", "button", "dialog", "textarea",
];
/// Page chrome, dropped unless the page's own `<main>`/`<article>` is read.
const CHROME: &[&str] = &["header", "footer"];
/// Elements that start and end a paragraph of their own.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "aside",
    "figure",
    "figcaption",
    "address",
    "details",
    "summary",
    "dl",
    "dt",
    "dd",
    "fieldset",
    "body",
    "caption",
];

/// What a page gave.
#[derive(Debug, PartialEq)]
pub struct Page {
    /// The page's own title, if it has a non-blank one.
    pub title: Option<String>,
    /// The text as Markdown, not yet tidied.
    pub body: String,
    /// Pictures (`<img>`) in the text that was read, which are left out.
    pub images_left_out: usize,
}

/// The charset a page declares in its first 4 KB (`<meta charset=…>` or
/// `content="text/html; charset=…"`), in lowercase.
pub(crate) fn declared_charset(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let at = head.find("charset")?;
    let rest = head[at + "charset".len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.trim_start_matches(['"', '\'']);
    let name: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// A saved page's text as a note body, its title and its picture count.
pub fn read(html: &str) -> Page {
    let tokens = tokenize(html);
    let title = title_of(&tokens);
    let mut rendered = None;
    if let Some((from, to)) = region(&tokens) {
        let (body, images) = render(&tokens[from..to], true, true);
        if !body.trim().is_empty() {
            rendered = Some((body, images));
        }
    }
    if rendered.is_none() {
        let (body, images) = render(&tokens, false, true);
        if !body.trim().is_empty() {
            rendered = Some((body, images));
        }
    }
    // Some sites wrap the whole page in one `<form>`: read it after all.
    let (body, images_left_out) = rendered.unwrap_or_else(|| render(&tokens, false, false));
    Page {
        title,
        body,
        images_left_out,
    }
}

enum Tok {
    Text(String),
    Open { name: String, raw: String },
    Close(String),
}

/// Splits `html` into text (entities decoded, comments gone) and tags.
fn tokenize(html: &str) -> Vec<Tok> {
    let lower = html.to_ascii_lowercase();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut index = 0;
    while index < html.len() {
        let rest = &html[index..];
        if rest.starts_with("<!--") {
            index += rest.find("-->").map_or(rest.len(), |end| end + 3);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            index += rest.find('>').map_or(rest.len(), |end| end + 1);
            continue;
        }
        if rest.starts_with('<') {
            if let Some((len, name, closing)) = parse_tag(rest) {
                if !text.is_empty() {
                    tokens.push(Tok::Text(std::mem::take(&mut text)));
                }
                let raw = rest[..len].to_string();
                index += len;
                if closing {
                    tokens.push(Tok::Close(name));
                } else {
                    let skip_content = RAW_TEXT.contains(&name.as_str()) && !raw.ends_with("/>");
                    let closer = format!("</{name}");
                    tokens.push(Tok::Open { name, raw });
                    if skip_content {
                        // Everything up to the closing tag is not text.
                        index = lower[index..]
                            .find(&closer)
                            .map_or(html.len(), |end| index + end);
                    }
                }
                continue;
            }
        }
        if rest.starts_with('&') {
            if let Some((ch, len)) = entity(rest) {
                text.push(ch);
                index += len;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or_default();
        text.push(ch);
        index += ch.len_utf8();
    }
    if !text.is_empty() {
        tokens.push(Tok::Text(text));
    }
    tokens
}

/// The Latin letters HTML names (the other entities are `anki.rs`'s).
const LETTERS: &[(&str, char)] = &[
    ("agrave", 'à'),
    ("aacute", 'á'),
    ("acirc", 'â'),
    ("atilde", 'ã'),
    ("auml", 'ä'),
    ("aring", 'å'),
    ("aelig", 'æ'),
    ("ccedil", 'ç'),
    ("egrave", 'è'),
    ("eacute", 'é'),
    ("ecirc", 'ê'),
    ("euml", 'ë'),
    ("igrave", 'ì'),
    ("iacute", 'í'),
    ("icirc", 'î'),
    ("iuml", 'ï'),
    ("ntilde", 'ñ'),
    ("ograve", 'ò'),
    ("oacute", 'ó'),
    ("ocirc", 'ô'),
    ("otilde", 'õ'),
    ("ouml", 'ö'),
    ("oslash", 'ø'),
    ("ugrave", 'ù'),
    ("uacute", 'ú'),
    ("ucirc", 'û'),
    ("uuml", 'ü'),
    ("yacute", 'ý'),
    ("yuml", 'ÿ'),
    ("szlig", 'ß'),
    ("Agrave", 'À'),
    ("Aacute", 'Á'),
    ("Acirc", 'Â'),
    ("Auml", 'Ä'),
    ("Aring", 'Å'),
    ("AElig", 'Æ'),
    ("Ccedil", 'Ç'),
    ("Egrave", 'È'),
    ("Eacute", 'É'),
    ("Ecirc", 'Ê'),
    ("Euml", 'Ë'),
    ("Igrave", 'Ì'),
    ("Iacute", 'Í'),
    ("Icirc", 'Î'),
    ("Iuml", 'Ï'),
    ("Ntilde", 'Ñ'),
    ("Ograve", 'Ò'),
    ("Oacute", 'Ó'),
    ("Ocirc", 'Ô'),
    ("Ouml", 'Ö'),
    ("Oslash", 'Ø'),
    ("Ugrave", 'Ù'),
    ("Uacute", 'Ú'),
    ("Ucirc", 'Û'),
    ("Uuml", 'Ü'),
    ("oelig", 'œ'),
];

/// The character an entity at the start of `text` (which starts with `&`)
/// stands for, and its length.
fn entity(text: &str) -> Option<(char, usize)> {
    if let Some(end) = text.find(';').filter(|end| *end <= 10) {
        let name = &text[1..end];
        if let Some((_, ch)) = LETTERS.iter().find(|(known, _)| *known == name) {
            return Some((*ch, end + 1));
        }
    }
    parse_entity(text)
}

/// `text` with its entities decoded, for attribute values.
fn decode_entities(text: &str) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with('&') {
            if let Some((ch, len)) = entity(rest) {
                out.push(ch);
                index += len;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or_default();
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// The value of attribute `wanted` in a start tag's text, entities decoded.
fn attr(tag: &str, wanted: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let len = bytes.len();
    let mut i = 1;
    // The tag's name.
    while i < len && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
        i += 1;
    }
    loop {
        while i < len && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i >= len || bytes[i] == b'>' {
            return None;
        }
        let start = i;
        while i < len && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'>' | b'/')
        {
            i += 1;
        }
        let name = &tag[start..i];
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = "";
        if i < len && bytes[i] == b'=' {
            i += 1;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let from = i;
                while i < len && bytes[i] != quote {
                    i += 1;
                }
                value = &tag[from..i];
                i = (i + 1).min(len);
            } else {
                let from = i;
                while i < len && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                value = &tag[from..i];
            }
        }
        if name.eq_ignore_ascii_case(wanted) {
            return Some(decode_entities(value));
        }
    }
}

/// `text` on one line, white space collapsed.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The page's title: `<title>`, else `og:title`, else the first `<h1>`.
fn title_of(tokens: &[Tok]) -> Option<String> {
    let non_blank = |text: String| Some(one_line(&text)).filter(|text| !text.is_empty());
    // `<title>`.
    for (index, token) in tokens.iter().enumerate() {
        if matches!(token, Tok::Open { name, .. } if name.as_str() == "title") {
            if let Some(Tok::Text(text)) = tokens.get(index + 1) {
                if let Some(title) = non_blank(text.clone()) {
                    return Some(title);
                }
            }
        }
    }
    // `og:title`.
    for token in tokens {
        if let Tok::Open { name, raw } = token {
            if name.as_str() == "meta" {
                let key = attr(raw, "property").or_else(|| attr(raw, "name"));
                if key.is_some_and(|key| key.eq_ignore_ascii_case("og:title")) {
                    if let Some(title) = attr(raw, "content").and_then(non_blank) {
                        return Some(title);
                    }
                }
            }
        }
    }
    // The first `<h1>`.
    let open = tokens
        .iter()
        .position(|token| matches!(token, Tok::Open { name, .. } if name.as_str() == "h1"))?;
    let mut text = String::new();
    for token in &tokens[open + 1..] {
        match token {
            Tok::Text(more) => {
                text.push_str(more);
                text.push(' ');
            }
            Tok::Close(name) if name.as_str() == "h1" => break,
            _ => {}
        }
    }
    non_blank(text)
}

/// The tokens inside the page's first `<main>`, else its first `<article>`:
/// where the content starts, and where its closing tag is.
fn region(tokens: &[Tok]) -> Option<(usize, usize)> {
    for wanted in ["main", "article"] {
        let Some(open) = tokens
            .iter()
            .position(|token| matches!(token, Tok::Open { name, .. } if name.as_str() == wanted))
        else {
            continue;
        };
        let mut depth = 0usize;
        for (offset, token) in tokens[open..].iter().enumerate() {
            match token {
                Tok::Open { name, .. } if name.as_str() == wanted => depth += 1,
                Tok::Close(name) if name.as_str() == wanted => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((open + 1, open + offset));
                    }
                }
                _ => {}
            }
        }
        // Never closed: to the end.
        return Some((open + 1, tokens.len()));
    }
    None
}

/// The Markdown for `tokens`, and how many pictures it left out.
/// `keep_chrome` reads `header`/`footer` too; `drop_forms` leaves out forms.
fn render(tokens: &[Tok], keep_chrome: bool, drop_forms: bool) -> (String, usize) {
    let mut out = Render::default();
    // The element being skipped, and how deeply it is nested in itself.
    let mut skip: Option<(String, usize)> = None;
    for token in tokens {
        if let Some((skipped, depth)) = &mut skip {
            let mut finished = false;
            match token {
                Tok::Open { name, raw }
                    if name.as_str() == skipped.as_str() && !raw.ends_with("/>") =>
                {
                    *depth += 1;
                }
                Tok::Close(name) if name.as_str() == skipped.as_str() => {
                    *depth -= 1;
                    finished = *depth == 0;
                }
                _ => {}
            }
            if finished {
                skip = None;
            }
            continue;
        }
        match token {
            Tok::Text(text) => out.text(text),
            Tok::Open { name, raw } => {
                let name = name.as_str();
                let dropped = ALWAYS_SKIPPED.contains(&name)
                    || (drop_forms && name == "form")
                    || (!keep_chrome && CHROME.contains(&name));
                if dropped {
                    if !raw.ends_with("/>") {
                        skip = Some((name.to_string(), 1));
                    }
                } else {
                    out.open(name, raw);
                }
            }
            Tok::Close(name) => out.close(name.as_str()),
        }
    }
    out.finish()
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
}

/// Builds the Markdown as tags and text go by.
#[derive(Default)]
struct Render {
    out: String,
    /// The last thing written was a list item, so the next one follows on the
    /// next line.
    last_item: bool,
    /// The paragraph being written.
    line: String,
    /// Inside `<pre>`: text is kept as it is.
    pre: bool,
    /// The level of the heading being written, or 0.
    heading: usize,
    quote: usize,
    /// Open lists: whether numbered, and the number of the last item.
    lists: Vec<(bool, usize)>,
    /// The marker the next paragraph starts with, and the spaces that
    /// continuing lines of the same item start with.
    marker: Option<String>,
    cont: String,
    /// Open links: the address (empty: none) and where the link's text starts
    /// in `line`.
    links: Vec<(String, usize)>,
    table: Option<Table>,
    table_depth: usize,
    in_cell: bool,
    images: usize,
}

impl Render {
    fn space(&mut self) {
        if !self.line.is_empty() && !self.line.ends_with([' ', '\n']) {
            self.line.push(' ');
        }
    }

    fn text(&mut self, text: &str) {
        if self.pre {
            self.line.push_str(text);
            return;
        }
        for ch in text.chars() {
            if ch.is_whitespace() {
                self.space();
            } else {
                self.line.push(ch);
            }
        }
    }

    fn line_break(&mut self) {
        if self.pre {
            self.line.push('\n');
        } else if self.in_cell || self.heading > 0 {
            self.space();
        } else {
            while self.line.ends_with(' ') {
                self.line.pop();
            }
            if !self.line.is_empty() && !self.line.ends_with('\n') {
                self.line.push('\n');
            }
        }
    }

    fn take_line(&mut self) -> String {
        self.links.clear();
        std::mem::take(&mut self.line)
    }

    fn emit(&mut self, block: &str, item: bool) {
        if block.is_empty() {
            return;
        }
        if !self.out.is_empty() {
            self.out
                .push_str(if item && self.last_item { "\n" } else { "\n\n" });
        }
        self.out.push_str(block);
        self.last_item = item;
    }

    /// Ends the paragraph being written (in a table cell: only separates).
    fn end_block(&mut self) {
        if self.in_cell {
            self.space();
        } else {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let raw = self.take_line();
        let lines: Vec<&str> = raw
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if lines.is_empty() {
            return;
        }
        let mut item = false;
        let mut text = if self.heading > 0 {
            format!("{} {}", "#".repeat(self.heading), lines.join(" "))
        } else {
            let marker = self.marker.take();
            item = marker.is_some();
            let first = marker.unwrap_or_else(|| self.cont.clone());
            let indent = " ".repeat(first.chars().count());
            lines
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let lead = if index == 0 { &first } else { &indent };
                    format!("{lead}{line}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        if self.quote > 0 {
            let quote = "> ".repeat(self.quote);
            text = text
                .lines()
                .map(|line| format!("{quote}{line}"))
                .collect::<Vec<_>>()
                .join("\n");
        }
        self.emit(&text, item);
    }

    fn emit_pre(&mut self) {
        let code = self.take_line();
        let code = code.trim_start_matches(['\n', '\r']).trim_end();
        if code.trim().is_empty() {
            return;
        }
        let fence = if code.contains("```") { "~~~~" } else { "```" };
        self.emit(&format!("{fence}\n{code}\n{fence}"), false);
    }

    fn open(&mut self, name: &str, raw: &str) {
        match name {
            "img" => self.images += 1,
            "br" => self.line_break(),
            "hr" => {
                self.end_block();
                if !self.in_cell {
                    self.emit("---", false);
                }
            }
            "a" => {
                let href = attr(raw, "href").unwrap_or_default();
                self.links.push((href, self.line.len()));
            }
            "code" | "kbd" | "samp" => {
                if !self.pre {
                    self.line.push('`');
                }
            }
            "pre" => {
                self.end_block();
                self.pre = !self.in_cell;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.end_block();
                if !self.in_cell {
                    self.heading = usize::from(name.as_bytes()[1] - b'0');
                }
            }
            "ul" | "ol" => {
                self.end_block();
                if !self.in_cell {
                    if self.lists.is_empty() {
                        // A new list, not more items of the one before.
                        self.last_item = false;
                    }
                    self.lists.push((name == "ol", 0));
                }
            }
            "li" => {
                self.end_block();
                if !self.in_cell {
                    if self.lists.is_empty() {
                        self.lists.push((false, 0));
                    }
                    let depth = self.lists.len();
                    let (ordered, number) = self.lists[depth - 1];
                    let number = number + 1;
                    self.lists[depth - 1].1 = number;
                    let marker = if ordered {
                        format!("{number}. ")
                    } else {
                        "- ".to_string()
                    };
                    let indent = "   ".repeat(depth - 1);
                    self.cont = " ".repeat(indent.len() + marker.len());
                    self.marker = Some(format!("{indent}{marker}"));
                }
            }
            "blockquote" => {
                self.end_block();
                if !self.in_cell {
                    self.quote += 1;
                }
            }
            "table" => {
                self.table_depth += 1;
                if self.table_depth == 1 {
                    self.end_block();
                    self.table = Some(Table::default());
                }
            }
            "tr" if self.table_depth == 1 => {
                // A cell or row may be left open (`</td>` is optional).
                if self.in_cell {
                    self.close("td");
                }
                self.close("tr");
                self.end_block();
            }
            "td" | "th" if self.table_depth == 1 => {
                if self.in_cell {
                    self.close("td");
                }
                self.end_block();
                self.in_cell = true;
            }
            _ if BLOCKS.contains(&name) => self.end_block(),
            _ => {}
        }
    }

    fn close(&mut self, name: &str) {
        match name {
            "a" => self.close_link(),
            "code" | "kbd" | "samp" => {
                if !self.pre {
                    self.line.push('`');
                }
            }
            "pre" => {
                if self.pre {
                    self.emit_pre();
                    self.pre = false;
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if self.heading > 0 {
                    self.flush();
                    self.heading = 0;
                } else {
                    self.end_block();
                }
            }
            "ul" | "ol" => {
                self.end_block();
                if !self.in_cell {
                    self.lists.pop();
                    if self.lists.is_empty() {
                        self.marker = None;
                        self.cont.clear();
                    }
                }
            }
            "li" => {
                self.end_block();
                if !self.in_cell {
                    self.marker = None;
                    self.cont.clear();
                }
            }
            "blockquote" => {
                self.end_block();
                if !self.in_cell && self.quote > 0 {
                    self.quote -= 1;
                }
            }
            "td" | "th" if self.table_depth == 1 && self.in_cell => {
                let cell = one_line(&self.take_line()).replace('|', "\\|");
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
                self.in_cell = false;
            }
            "tr" if self.table_depth == 1 => {
                if let Some(table) = &mut self.table {
                    if !table.row.is_empty() {
                        let row = std::mem::take(&mut table.row);
                        table.rows.push(row);
                    }
                }
            }
            "table" => {
                if self.table_depth == 1 {
                    if self.in_cell {
                        self.close("td");
                    }
                    self.close("tr");
                    self.close_table();
                }
                self.table_depth = self.table_depth.saturating_sub(1);
            }
            _ if BLOCKS.contains(&name) => self.end_block(),
            _ => {}
        }
    }

    /// `[text](address)` for a link with an address worth keeping; the address
    /// is written as text and never followed.
    fn close_link(&mut self) {
        let Some((href, start)) = self.links.pop() else {
            return;
        };
        let href = href.trim();
        let lower = href.to_ascii_lowercase();
        if self.pre
            || href.is_empty()
            || href.starts_with('#')
            || lower.starts_with("javascript:")
            || lower.starts_with("data:")
        {
            return;
        }
        let start = start.min(self.line.len());
        if !self.line.is_char_boundary(start) {
            return;
        }
        let label = self.line.split_off(start);
        let trimmed = label.trim();
        if trimmed.is_empty() {
            self.line.push_str(&label);
            return;
        }
        let address = href
            .replace(' ', "%20")
            .replace('(', "%28")
            .replace(')', "%29");
        if label.starts_with(char::is_whitespace) {
            self.line.push(' ');
        }
        self.line.push_str(&format!("[{trimmed}]({address})"));
        if label.ends_with(char::is_whitespace) {
            self.line.push(' ');
        }
    }

    /// Writes the table just closed as a pipe table, every row padded to the
    /// widest, the first row as the header.
    fn close_table(&mut self) {
        let Some(mut table) = self.table.take() else {
            return;
        };
        self.in_cell = false;
        if !table.row.is_empty() {
            let row = std::mem::take(&mut table.row);
            table.rows.push(row);
        }
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let write = |row: &[String]| -> String {
            let mut cells: Vec<&str> = row.iter().map(String::as_str).collect();
            while cells.len() < columns {
                cells.push("");
            }
            format!("| {} |", cells.join(" | "))
        };
        let mut lines = vec![
            write(&table.rows[0]),
            format!("|{}|", " --- |".repeat(columns).trim_end_matches('|')),
        ];
        for row in &table.rows[1..] {
            lines.push(write(row));
        }
        self.emit(&lines.join("\n"), false);
    }

    fn finish(mut self) -> (String, usize) {
        if self.pre {
            self.emit_pre();
        }
        if self.table.is_some() {
            self.close_table();
        }
        self.flush();
        (self.out, self.images)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE: &[u8] = include_bytes!("../testdata/html/article.html");
    const ITALIAN: &[u8] = include_bytes!("../testdata/html/italiano.html");
    const UNTITLED: &[u8] = include_bytes!("../testdata/html/untitled.html");

    fn page(bytes: &[u8]) -> Page {
        read(&String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[test]
    fn an_article_page_loses_its_chrome_and_keeps_its_structure() {
        let page = page(ARTICLE);
        assert_eq!(page.title.as_deref(), Some("Mitosis explained"));
        let body = page.body;
        for gone in [
            "Home",
            "Subscribe",
            "tracker",
            "Copyright",
            "hidden comment",
            "noscript text",
        ] {
            assert!(!body.contains(gone), "{gone} in {body:?}");
        }
        assert!(body.starts_with("# Mitosis explained\n\n"), "{body:?}");
        assert!(body.contains("## Phases\n\n"), "{body:?}");
        assert!(
            body.contains("- Prophase: chromosomes condense\n- Metaphase: they line up"),
            "{body:?}"
        );
        assert!(body.contains("1. First\n2. Second"), "{body:?}");
        assert!(body.contains("> Omnis cellula e cellula."), "{body:?}");
        assert!(
            body.contains("```\nlet cells = 2;\n  cells *= 2;\n```"),
            "{body:?}"
        );
        assert!(
            body.contains("| Phase | Event |\n| --- | --- |\n| Prophase | Condense |"),
            "{body:?}"
        );
        assert!(
            body.contains("[the textbook](https://example.org/book)"),
            "{body:?}"
        );
        assert!(body.contains("`DNA`"), "{body:?}");
        // The picture is left out and counted.
        assert!(!body.contains("diagram.png"), "{body:?}");
        assert_eq!(page.images_left_out, 1);
    }

    #[test]
    fn an_italian_page_keeps_its_accents_and_decodes_entities() {
        let page = page(ITALIAN);
        assert_eq!(
            page.title.as_deref(),
            Some("La cellula è l'unità della vita")
        );
        assert!(
            page.body
                .contains("Perché le cellule si dividono? Più o meno così: città & «mitosi»."),
            "{:?}",
            page.body
        );
        assert!(page.body.contains("caffè — 5 €"), "{:?}", page.body);
    }

    #[test]
    fn a_page_without_a_title_uses_og_title_then_the_first_heading() {
        let og = "<html><head><meta property=\"og:title\" content=\"Open &amp; shut\"></head>\
                  <body><p>Text</p></body></html>";
        assert_eq!(read(og).title.as_deref(), Some("Open & shut"));
        let page = page(UNTITLED);
        assert_eq!(page.title.as_deref(), Some("Only a heading"));
        assert!(page.body.contains("Body text."), "{:?}", page.body);
        assert_eq!(read("<p>No title at all</p>").title, None);
    }

    #[test]
    fn main_content_is_preferred_and_keeps_its_own_header() {
        let html = "<body><nav>Menu</nav><main><header><h1>Title</h1></header>\
                    <p>Text</p><footer>Foot</footer></main><footer>Site</footer></body>";
        let page = read(html);
        assert!(page.body.contains("# Title"), "{:?}", page.body);
        assert!(!page.body.contains("Menu") && !page.body.contains("Site"));
    }

    #[test]
    fn a_page_wrapped_in_a_form_is_still_read() {
        let page = read("<body><form><p>All the text</p></form></body>");
        assert_eq!(page.body, "All the text");
    }

    #[test]
    fn scripts_with_angle_brackets_do_not_break_reading() {
        let html =
            "<p>Before</p><script>if (a < b && c > d) { x = '<p>no</p>'; }</script><p>After</p>";
        assert_eq!(read(html).body, "Before\n\nAfter");
    }

    #[test]
    fn links_without_a_usable_address_stay_plain_text() {
        let html = "<p><a href=\"#top\">Top</a> <a href=\"javascript:x()\">Run</a> \
                    <a href=\"/a b\">Rel</a> <a href=\"x\"><img src=\"p.png\"></a></p>";
        let page = read(html);
        assert_eq!(page.body, "Top Run [Rel](/a%20b)");
        assert_eq!(page.images_left_out, 1);
    }

    #[test]
    fn the_declared_charset_is_found() {
        assert_eq!(
            declared_charset(b"<meta charset=\"UTF-8\">").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            declared_charset(b"<meta http-equiv=x content='text/html; charset=ISO-8859-1'>")
                .as_deref(),
            Some("iso-8859-1")
        );
        assert_eq!(declared_charset(b"<p>none</p>"), None);
    }
}
