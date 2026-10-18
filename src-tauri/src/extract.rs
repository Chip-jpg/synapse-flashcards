//! Reading the text of a file the user imports as a note: PDF, plain text,
//! Markdown, Word (`.docx`), or PowerPoint (`.pptx`).
//!
//! An import becomes an ordinary note. Only the text is taken, and the file
//! itself isn't kept: Synapse doesn't copy it, link to it, or remember where
//! it was. The user reads the text in the new-note form and saves it (or
//! doesn't) like a note they typed, so the database and backups don't change.
//!
//! What's read from each kind of file:
//! - Text and Markdown: the whole file, as UTF-8 (or UTF-16 with a byte order
//!   mark), and otherwise as Windows-1252, which older Windows programs write.
//!   Markdown stays as written.
//! - Word: the paragraphs of the document's body, one per line, tables
//!   included. Headers, footers, footnotes, comments, and images are left out.
//! - PowerPoint: each slide's text in slide order, under "Slide 1" and so on,
//!   then its speaker notes. Images are left out.
//! - PDF: each page's text, with lines broken at the edge of the page joined
//!   up again ([`unwrap_lines`]). Pages with no text are reported, since
//!   they're usually scanned images (there's no OCR), and a PDF with no text
//!   at all is refused.
//!
//! PDFs are read by a second, short-lived run of Synapse ([`PDF_HELPER_ARG`]).
//! The PDF library can fail on a damaged or unusual file by panicking, which
//! in a release build would close Synapse, or by never finishing. A helper
//! that crashes or runs past [`PDF_TIMEOUT`] is simply a file that couldn't be
//! read, and the app carries on.
//!
//! Nothing here logs a file's text.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::db::DbError;
use crate::notes::NOTE_TITLE_MAX_CHARS;

/// The largest file that can be imported. Text this long would be far more
/// than a note can hold anyway; the limit keeps reading quick.
pub const IMPORT_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// How long the PDF helper may take before it's stopped.
pub const PDF_TIMEOUT: Duration = Duration::from_secs(60);

/// The argument that makes Synapse run as the PDF helper (see `main.rs`).
pub const PDF_HELPER_ARG: &str = "--read-pdf-text";

/// The most one XML part of a Word or PowerPoint file may unpack to: far more
/// than any real document, but a bound on a file built to unpack forever.
const XML_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The kinds of file that can be imported, by their extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    Pdf,
    Text,
    Markdown,
    Word,
    PowerPoint,
}

impl FileKind {
    /// The extensions offered in the file window, without the dot.
    pub const EXTENSIONS: [&'static str; 6] = ["pdf", "txt", "md", "markdown", "docx", "pptx"];

    /// The kind of file `path` is, by its extension (any letter case).
    pub fn of(path: &Path) -> Option<FileKind> {
        let extension = path.extension()?.to_str()?.to_lowercase();
        Some(match extension.as_str() {
            "pdf" => FileKind::Pdf,
            "txt" => FileKind::Text,
            "md" | "markdown" => FileKind::Markdown,
            "docx" => FileKind::Word,
            "pptx" => FileKind::PowerPoint,
            _ => return None,
        })
    }
}

/// The text read from a file, ready to become a note:
/// `{ kind, title, body, pages, pagesWithoutText }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedText {
    pub kind: FileKind,
    /// A title for the note: the document's own title if it has one, else the
    /// file's name without its extension. At most a note title's length.
    pub title: String,
    /// The text, with line breaks as `\n`, and runs of blank lines (and
    /// spacing at the very start and end) taken out.
    pub body: String,
    /// Pages of a PDF or slides of a presentation; none for other files.
    pub pages: Option<usize>,
    /// The pages of a PDF (counting from 1) that had no text.
    pub pages_without_text: Vec<usize>,
}

/// Why a file couldn't be imported. Detail is for logs only, and never holds
/// the file's text.
#[derive(Debug)]
pub enum ExtractError {
    /// Not one of the kinds in [`FileKind`].
    Unsupported,
    /// Larger than [`IMPORT_MAX_BYTES`].
    TooLarge,
    /// The file couldn't be read from disk.
    Unreadable(DbError),
    /// The file isn't a readable file of its kind: damaged, misnamed, or (for
    /// a PDF) password-protected.
    Damaged(DbError),
    /// The file has no text to import (a scanned PDF, an empty document).
    NoText,
    /// The PDF helper crashed, couldn't start, or ran too long.
    HelperFailed(DbError),
}

/// Reads the text of the file at `path`, starting `helper` (normally Synapse
/// itself) to read a PDF. Blocking: run it off the async runtime.
pub fn read_file(path: &Path, helper: &Path) -> Result<ExtractedText, ExtractError> {
    let kind = FileKind::of(path).ok_or(ExtractError::Unsupported)?;
    let size = std::fs::metadata(path)
        .map_err(|err| ExtractError::Unreadable(err.into()))?
        .len();
    if size > IMPORT_MAX_BYTES {
        return Err(ExtractError::TooLarge);
    }
    let bytes = std::fs::read(path).map_err(|err| ExtractError::Unreadable(err.into()))?;
    let file_title = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    let (own_title, body, pages, pages_without_text) = match kind {
        FileKind::Text => (None, decode_text(&bytes), None, Vec::new()),
        FileKind::Markdown => {
            let text = decode_text(&bytes).replace("\r\n", "\n");
            (markdown_title(&text), text, None, Vec::new())
        }
        FileKind::Word => {
            let (title, body) = word_text(&bytes)?;
            (title, body, None, Vec::new())
        }
        FileKind::PowerPoint => {
            let (title, body, slides) = powerpoint_text(&bytes)?;
            (title, body, Some(slides), Vec::new())
        }
        FileKind::Pdf => {
            let texts = read_pdf(helper, &bytes)?;
            let empty = texts
                .iter()
                .enumerate()
                .filter(|(_, text)| text.trim().is_empty())
                .map(|(index, _)| index + 1)
                .collect();
            let text: Vec<String> = texts.iter().map(|page| unwrap_lines(page)).collect();
            (None, text.join("\n\n"), Some(texts.len()), empty)
        }
    };

    let body = tidy(&body);
    if body.is_empty() {
        return Err(ExtractError::NoText);
    }
    Ok(ExtractedText {
        kind,
        title: note_title(own_title.as_deref().unwrap_or(&file_title)),
        body,
        pages,
        pages_without_text,
    })
}

/// Joins up the lines of a PDF page that were only broken to fit its width.
/// A line runs on into the next one unless it ends a sentence or clause
/// (`.`, `!`, `?`, `:`, `;`), or the next one starts a list item, and blank
/// lines between paragraphs stay. A line ending in a hyphen joins without a
/// space, the hyphen kept, since it may belong to the word.
fn unwrap_lines(page: &str) -> String {
    let starts_item = |line: &str| {
        let line = line.trim_start();
        let numbered = line.split_once(['.', ')']).is_some_and(|(number, _)| {
            !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
        });
        numbered
            || ["- ", "• ", "* ", "– "]
                .iter()
                .any(|marker| line.starts_with(marker))
    };
    let mut out = String::new();
    let mut previous: Option<&str> = None;
    for line in page.lines() {
        let line = line.trim_end();
        match previous {
            Some(before)
                if !before.is_empty()
                    && !line.trim().is_empty()
                    && !before.ends_with(['.', '!', '?', ':', ';'])
                    && !starts_item(line) =>
            {
                if !before.ends_with('-') {
                    out.push(' ');
                }
                out.push_str(line.trim_start());
            }
            Some(_) => {
                out.push('\n');
                out.push_str(line);
            }
            None => out.push_str(line),
        }
        previous = Some(line);
    }
    out
}

/// A title as a note can hold it: trimmed, on one line, at most
/// [`NOTE_TITLE_MAX_CHARS`], and never blank.
fn note_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = title.chars().take(NOTE_TITLE_MAX_CHARS).collect();
    let title = title.trim();
    if title.is_empty() {
        "Imported note".to_string()
    } else {
        title.to_string()
    }
}

/// Tidies extracted text: line breaks become `\n`, control characters other
/// than tabs go, spacing at the end of each line goes, no more than one blank
/// line is kept in a row, and the start and end are trimmed.
fn tidy(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line: String = line
            .chars()
            .filter(|c| *c == '\t' || !c.is_control())
            .collect();
        let line = line.trim_end().to_string();
        if line.is_empty() && lines.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// A text file's characters: UTF-8 or UTF-16 if its byte order mark says so,
/// UTF-8 if it is valid UTF-8, and otherwise Windows-1252.
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], from: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| from([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, u16::from_be_bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&byte| windows_1252(byte)).collect(),
    }
}

/// One Windows-1252 byte as a character. It's Latin-1 except for 0x80–0x9F,
/// where it has typographic characters such as curly quotes and dashes.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// A Markdown file's title: the `title:` in its front matter (the `---`
/// block many note apps start a file with), or else its first line, if that's
/// a `# Heading`.
fn markdown_title(text: &str) -> Option<String> {
    let mut text = text.trim_start();
    if let Some((matter, after)) = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
    {
        let title = matter.lines().find_map(|line| line.strip_prefix("title:"));
        if let Some(title) = title.map(|title| title.trim().trim_matches(['"', '\'']).trim()) {
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
        // The rest of the closing `---` line goes too.
        text = after
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .trim_start();
    }
    let first = text.lines().next()?;
    let heading = first
        .strip_prefix("# ")?
        .trim()
        .trim_end_matches('#')
        .trim();
    (!heading.is_empty()).then(|| heading.to_string())
}

/// The archive inside a Word or PowerPoint file.
type Package<'a> = zip::ZipArchive<std::io::Cursor<&'a [u8]>>;

fn open_package(bytes: &[u8]) -> Result<Package<'_>, ExtractError> {
    zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| ExtractError::Damaged(format!("not a zip package: {err}").into()))
}

/// One XML part of the package, or `None` if it has no such part.
fn package_part(package: &mut Package<'_>, name: &str) -> Result<Option<String>, ExtractError> {
    let entry = match package.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => return Err(ExtractError::Damaged(format!("{name}: {err}").into())),
    };
    let mut xml = String::new();
    entry
        .take(XML_MAX_BYTES + 1)
        .read_to_string(&mut xml)
        .map_err(|err| ExtractError::Damaged(format!("{name}: {err}").into()))?;
    if xml.len() as u64 > XML_MAX_BYTES {
        return Err(ExtractError::Damaged(format!("{name} is too large").into()));
    }
    Ok(Some(xml))
}

/// Which XML elements mean what, for [`xml_text`]. Names are local (without
/// a namespace prefix). Text in any other element is left out: in Word, that
/// includes deleted text in tracked changes (`delText`) and field codes
/// (`instrText`).
struct Markup {
    /// Elements whose text is the document's text.
    text: &'static [&'static str],
    /// Elements that end a paragraph.
    paragraph: &'static [&'static str],
    /// Empty elements that stand for a line break or a tab.
    line_break: &'static [&'static str],
    tab: &'static [&'static str],
    /// Elements that make their paragraph a list item, which then starts
    /// with "- ". (Word works out list numbers itself, so they aren't there.)
    list_item: &'static [&'static str],
    /// Table cells and rows: a row's cells go on one line, between tabs.
    cell: &'static [&'static str],
    row: &'static [&'static str],
}

const WORD: Markup = Markup {
    text: &["t"],
    paragraph: &["p"],
    line_break: &["br", "cr"],
    tab: &["tab"],
    list_item: &["numPr"],
    cell: &["tc"],
    row: &["tr"],
};

const DRAWING: Markup = Markup {
    text: &["t"],
    paragraph: &["p"],
    line_break: &["br"],
    tab: &[],
    list_item: &[],
    cell: &["tc"],
    row: &["tr"],
};

/// A document's properties (`docProps/core.xml`), where its title is.
const PROPERTIES: Markup = Markup {
    text: &["title"],
    paragraph: &[],
    line_break: &[],
    tab: &[],
    list_item: &[],
    cell: &[],
    row: &[],
};

/// The text of one XML part, laid out as paragraphs (one per line).
fn xml_text(xml: &str, markup: &Markup) -> Result<String, ExtractError> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    // How deep inside text elements we are.
    let mut in_text = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|err| ExtractError::Damaged(format!("XML: {err}").into()))?;
        match event {
            Event::Start(element) => {
                let name = element.local_name();
                let name: &str = name.as_ref();
                if markup.text.contains(&name) {
                    in_text += 1;
                } else if markup.list_item.contains(&name) {
                    // Paragraph properties come before its text.
                    out.push_str("- ");
                }
            }
            Event::End(element) => {
                let name = element.local_name();
                let name: &str = name.as_ref();
                if markup.text.contains(&name) {
                    in_text = in_text.saturating_sub(1);
                } else if markup.paragraph.contains(&name) {
                    out.push('\n');
                } else if markup.cell.contains(&name) {
                    // The cell's last paragraph ended the line: the row goes on.
                    if out.ends_with('\n') {
                        out.pop();
                    }
                    out.push('\t');
                } else if markup.row.contains(&name) {
                    if out.ends_with('\t') {
                        out.pop();
                    }
                    out.push('\n');
                }
            }
            Event::Empty(element) => {
                let name = element.local_name();
                let name: &str = name.as_ref();
                if markup.list_item.contains(&name) {
                    out.push_str("- ");
                } else if markup.line_break.contains(&name) {
                    out.push('\n');
                } else if markup.tab.contains(&name) {
                    out.push('\t');
                } else if markup.paragraph.contains(&name) {
                    out.push('\n');
                }
            }
            Event::Text(text) if in_text > 0 => {
                out.push_str(&text.xml10_content());
            }
            Event::CData(text) if in_text > 0 => {
                out.push_str(&text.into_inner());
            }
            Event::GeneralRef(reference) if in_text > 0 => {
                if let Ok(Some(character)) = reference.resolve_char_ref() {
                    out.push(character);
                } else if let Some(entity) =
                    quick_xml::escape::resolve_predefined_entity(&reference)
                {
                    out.push_str(entity);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// The value of attribute `name` (a local name) on an element, if it has it.
fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        (attribute.key.local_name().as_ref() == name).then(|| attribute.value.into_owned())
    })
}

/// A Word or PowerPoint file's own title, from its properties, if it has one.
fn package_title(package: &mut Package<'_>) -> Result<Option<String>, ExtractError> {
    Ok(package_part(package, "docProps/core.xml")?
        .and_then(|core| xml_text(&core, &PROPERTIES).ok())
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty()))
}

/// A Word document's own title, if it has one, and the text of its body.
fn word_text(bytes: &[u8]) -> Result<(Option<String>, String), ExtractError> {
    let mut package = open_package(bytes)?;
    let document = package_part(&mut package, "word/document.xml")?
        .ok_or_else(|| ExtractError::Damaged("no word/document.xml".into()))?;
    let body = xml_text(&document, &WORD)?;
    Ok((package_title(&mut package)?, body))
}

/// Where a relationship (`rId…`) in a `.rels` part points, as a path in the
/// package, resolved against the folder of the part that owns it.
fn relationship_targets(
    rels: &str,
    folder: &str,
) -> Result<Vec<(String, String, String)>, ExtractError> {
    let mut reader = Reader::from_str(rels);
    let mut targets = Vec::new();
    loop {
        match reader
            .read_event()
            .map_err(|err| ExtractError::Damaged(format!("relationships: {err}").into()))?
        {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == "Relationship" =>
            {
                let (Some(id), Some(target)) =
                    (attribute(&element, "Id"), attribute(&element, "Target"))
                else {
                    continue;
                };
                let kind = attribute(&element, "Type").unwrap_or_default();
                targets.push((id, resolve(folder, &target), kind));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(targets)
}

/// `target` (relative, as in a `.rels` part) resolved against `folder`.
fn resolve(folder: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    } else {
        folder.split('/').filter(|part| !part.is_empty()).collect()
    };
    for part in target.split('/') {
        match part {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// A presentation's own title, if it has one; its text, slide by slide in the
/// order they're shown, each under "Slide N" and followed by its speaker
/// notes; and how many slides it has.
fn powerpoint_text(bytes: &[u8]) -> Result<(Option<String>, String, usize), ExtractError> {
    let mut package = open_package(bytes)?;
    let presentation = package_part(&mut package, "ppt/presentation.xml")?
        .ok_or_else(|| ExtractError::Damaged("no ppt/presentation.xml".into()))?;
    let rels = package_part(&mut package, "ppt/_rels/presentation.xml.rels")?
        .ok_or_else(|| ExtractError::Damaged("no presentation relationships".into()))?;
    let targets = relationship_targets(&rels, "ppt")?;

    // The slide order is the order of `<p:sldId r:id="…"/>` in the presentation.
    let mut slide_paths = Vec::new();
    let mut reader = Reader::from_str(&presentation);
    loop {
        match reader
            .read_event()
            .map_err(|err| ExtractError::Damaged(format!("presentation: {err}").into()))?
        {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == "sldId" =>
            {
                // Its `r:id`, not its own numeric `id`: both are "id" without
                // their namespace prefix.
                let link = element.attributes().flatten().find_map(|attribute| {
                    (attribute.key.local_name().as_ref() == "id"
                        && attribute.key.prefix().is_some())
                    .then(|| attribute.value.into_owned())
                });
                let path = link
                    .and_then(|id| targets.iter().find(|(rid, _, _)| *rid == id))
                    .map(|(_, path, _)| path.clone());
                if let Some(path) = path {
                    slide_paths.push(path);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let mut out = String::new();
    for (number, path) in slide_paths.iter().enumerate() {
        let slide = package_part(&mut package, path)?
            .ok_or_else(|| ExtractError::Damaged(format!("missing {path}").into()))?;
        out.push_str(&format!("Slide {}\n", number + 1));
        out.push_str(&xml_text(&slide, &DRAWING)?);

        let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
        let notes = match package_part(&mut package, &format!("{folder}/_rels/{file}.rels"))? {
            Some(rels) => relationship_targets(&rels, folder)?
                .into_iter()
                .find(|(_, _, kind)| kind.ends_with("/notesSlide"))
                .map(|(_, notes_path, _)| notes_path),
            None => None,
        };
        if let Some(notes_path) = notes {
            if let Some(notes) = package_part(&mut package, &notes_path)? {
                let notes = speaker_notes(&notes)?;
                if !notes.trim().is_empty() {
                    out.push_str("\nSpeaker notes:\n");
                    out.push_str(&notes);
                }
            }
        }
        out.push_str("\n\n");
    }
    Ok((package_title(&mut package)?, out, slide_paths.len()))
}

/// The speaker notes on a notes page: the text of its body placeholder only,
/// not the slide number or the picture of the slide.
fn speaker_notes(xml: &str) -> Result<String, ExtractError> {
    // Each shape (`<p:sp>`) is read on its own, and kept if its placeholder
    // (`<p:ph>`) is the body.
    let mut kept = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<p:sp>").or_else(|| rest.find("<p:sp ")) {
        let Some(length) = rest[start..].find("</p:sp>") else {
            break;
        };
        let shape = &rest[start..start + length + "</p:sp>".len()];
        if shape.contains("type=\"body\"") {
            kept.push_str(&xml_text(shape, &DRAWING)?);
        }
        rest = &rest[start + length..];
    }
    Ok(kept)
}

/// What the PDF helper sends back on standard output: the text of each page,
/// or why the PDF couldn't be read.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum HelperAnswer {
    Pages(Vec<String>),
    Unreadable(String),
}

/// Runs as the PDF helper: reads a PDF from standard input, writes a
/// [`HelperAnswer`] to standard output, and returns the exit code. A panic in
/// the PDF library ends the process instead, which the app reports as a file
/// it couldn't read.
pub fn pdf_helper() -> i32 {
    let mut bytes = Vec::new();
    if std::io::stdin()
        .take(IMPORT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return 2;
    }
    let answer = match pdf_extract::extract_text_from_mem_by_pages(&bytes) {
        Ok(pages) => HelperAnswer::Pages(pages),
        Err(err) => HelperAnswer::Unreadable(err.to_string()),
    };
    let mut out = std::io::stdout().lock();
    let written = serde_json::to_writer(&mut out, &answer).is_ok() && out.flush().is_ok();
    if written {
        0
    } else {
        3
    }
}

/// Has `helper` (Synapse, run with [`PDF_HELPER_ARG`]) read the text of each
/// page of a PDF, stopping it if it takes longer than [`PDF_TIMEOUT`].
fn read_pdf(helper: &Path, pdf: &[u8]) -> Result<Vec<String>, ExtractError> {
    read_pdf_within(helper, pdf, PDF_TIMEOUT)
}

/// [`read_pdf`], with its time limit given.
pub fn read_pdf_within(
    helper: &Path,
    pdf: &[u8],
    timeout: Duration,
) -> Result<Vec<String>, ExtractError> {
    let failed = |detail: String| ExtractError::HelperFailed(detail.into());
    let mut command = Command::new(helper);
    command
        .arg(PDF_HELPER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window flashes up while it works.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|err| failed(format!("starting the helper: {err}")))?;

    // Writing and reading on their own threads, so neither pipe can fill up
    // and stall the other while the helper works.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let input = pdf.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut answer = Vec::new();
        stdout.read_to_end(&mut answer).map(|_| answer)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                // Its pipes close with it, so both threads finish.
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(format!("the helper ran past {timeout:?}")));
            }
            Err(err) => return Err(failed(format!("waiting for the helper: {err}"))),
        }
    };
    // A helper that exits early may not read all its input; that's reported below.
    let _ = writer.join();
    let answer = reader
        .join()
        .map_err(|_| failed("reading the helper's answer panicked".to_string()))?
        .map_err(|err| failed(format!("reading the helper's answer: {err}")))?;
    if !status.success() {
        return Err(failed(format!("the helper exited with {status}")));
    }
    match serde_json::from_slice(&answer) {
        Ok(HelperAnswer::Pages(pages)) => Ok(pages),
        Ok(HelperAnswer::Unreadable(detail)) => Err(ExtractError::Damaged(detail.into())),
        Err(err) => Err(failed(format!("the helper's answer: {err}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip package with the given parts, as Word and PowerPoint files are.
    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in parts {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// Writes `bytes` to a new file named `name` in a fresh folder, and reads
    /// it as an import. PDFs aren't read here (see `tests/pdf_helper.rs`).
    fn import(name: &str, bytes: &[u8]) -> Result<ExtractedText, ExtractError> {
        // Tests run in parallel, so every import gets a folder of its own.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let folder = std::env::temp_dir().join(format!(
            "synapse-extract-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        std::fs::write(&path, bytes).unwrap();
        let result = read_file(&path, Path::new("no-helper"));
        std::fs::remove_dir_all(&folder).unwrap();
        result
    }

    const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";

    fn word(body: &str, title: Option<&str>) -> Vec<u8> {
        let document =
            format!("<?xml version=\"1.0\"?><w:document {W}><w:body>{body}</w:body></w:document>");
        let mut parts = vec![("word/document.xml", document.as_str())];
        let core = title.map(|title| {
            format!(
                "<cp:coreProperties xmlns:cp=\"cp\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
                 <dc:title>{title}</dc:title></cp:coreProperties>"
            )
        });
        if let Some(core) = &core {
            parts.push(("docProps/core.xml", core.as_str()));
        }
        package(&parts)
    }

    #[test]
    fn kinds_are_known_by_their_extension_in_any_case() {
        for (name, kind) in [
            ("a.pdf", Some(FileKind::Pdf)),
            ("a.PDF", Some(FileKind::Pdf)),
            ("notes.txt", Some(FileKind::Text)),
            ("notes.md", Some(FileKind::Markdown)),
            ("notes.markdown", Some(FileKind::Markdown)),
            ("essay.DocX", Some(FileKind::Word)),
            ("talk.pptx", Some(FileKind::PowerPoint)),
            ("old.doc", None),
            ("old.ppt", None),
            ("picture.png", None),
            ("no-extension", None),
        ] {
            assert_eq!(FileKind::of(Path::new(name)), kind, "{name}");
        }
        assert!(matches!(
            import("old.doc", b"x"),
            Err(ExtractError::Unsupported)
        ));
    }

    #[test]
    fn text_files_are_read_as_utf8_utf16_or_windows_1252() {
        let read = |bytes: &[u8]| import("notes.txt", bytes).unwrap().body;
        assert_eq!(
            read("Café – naïve\r\nline two".as_bytes()),
            "Café – naïve\nline two"
        );
        assert_eq!(read(b"\xEF\xBB\xBFWith a BOM"), "With a BOM");
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("Ünïcode".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(read(&utf16), "Ünïcode");
        // Not valid UTF-8: Windows-1252, with its curly quotes and dash.
        assert_eq!(read(b"\x93Caf\xe9\x94 \x96 \x80"), "“Café” – €");
    }

    #[test]
    fn text_is_tidied_and_titled_by_the_file_name() {
        let read = import(
            "Cell biology.txt",
            b"\n\n  Mitosis   \n\n\n\nhas\tphases\x07\n\n",
        )
        .unwrap();
        assert_eq!(
            read,
            ExtractedText {
                kind: FileKind::Text,
                title: "Cell biology".to_string(),
                body: "Mitosis\n\nhas\tphases".to_string(),
                pages: None,
                pages_without_text: Vec::new(),
            }
        );
    }

    #[test]
    fn markdown_is_kept_as_written_and_titled_by_its_first_heading() {
        let text = "# Mitosis #\n\n- **Prophase**: chromosomes condense\n";
        let read = import("cells.md", text.as_bytes()).unwrap();
        assert_eq!(read.title, "Mitosis");
        assert_eq!(
            read.body,
            "# Mitosis #\n\n- **Prophase**: chromosomes condense"
        );
        // No heading at the top: the file name.
        assert_eq!(
            import("cells.md", b"Some text\n# Later").unwrap().title,
            "cells"
        );
        // Front matter: its title, or else the heading after it. It stays in
        // the text.
        let matter = "---\r\ntags: [bio]\r\ntitle: \"Cell division\"\r\n---\r\n\r\n# Mitosis\r\n";
        let read = import("cells.md", matter.as_bytes()).unwrap();
        assert_eq!(read.title, "Cell division");
        assert!(
            read.body.starts_with("---\ntags: [bio]\n"),
            "{:?}",
            read.body
        );
        let untitled = "---\ntags: [bio]\n---\n# Mitosis\nText";
        assert_eq!(
            import("cells.md", untitled.as_bytes()).unwrap().title,
            "Mitosis"
        );
    }

    #[test]
    fn a_word_document_gives_its_paragraphs_and_its_own_title() {
        let body = "<w:p><w:r><w:t>Mitosis</w:t></w:r></w:p>\
            <w:p><w:r><w:t xml:space=\"preserve\">Cells </w:t></w:r><w:r><w:t>divide &amp; grow</w:t></w:r>\
            <w:r><w:tab/><w:t>&#8212;</w:t><w:br/><w:t>new line</w:t></w:r></w:p>\
            <w:p><w:r><w:delText>deleted words</w:delText></w:r><w:r><w:t>kept</w:t></w:r></w:p>\
            <w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>\
            <w:r><w:t>A list item</w:t></w:r></w:p>\
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Phase</w:t></w:r></w:p></w:tc>\
            <w:tc><w:p><w:r><w:t>Metaphase</w:t></w:r></w:p></w:tc></w:tr>\
            <w:tr><w:tc><w:p><w:r><w:t>Event</w:t></w:r></w:p></w:tc>\
            <w:tc><w:p><w:r><w:t>Line up</w:t></w:r></w:p></w:tc></w:tr></w:tbl>\
            <w:p/><w:p><w:r><w:t>End</w:t></w:r></w:p>";
        let read = import("lecture.docx", &word(body, Some(" Cell division "))).unwrap();
        assert_eq!(read.kind, FileKind::Word);
        assert_eq!(read.title, "Cell division");
        assert_eq!(
            read.body,
            "Mitosis\nCells divide & grow\t—\nnew line\nkept\n- A list item\n\
             Phase\tMetaphase\nEvent\tLine up\n\nEnd"
        );
        // No title in its properties: the file name.
        let untitled = import(
            "lecture.docx",
            &word("<w:p><w:r><w:t>x</w:t></w:r></w:p>", None),
        );
        assert_eq!(untitled.unwrap().title, "lecture");
    }

    /// A presentation whose slides are stored in a different order from the
    /// one they're shown in, with speaker notes on the second slide shown.
    fn presentation() -> Vec<u8> {
        const P: &str = "xmlns:p=\"p\" xmlns:a=\"a\" xmlns:r=\"r\"";
        let slide = |text: &str| {
            format!(
                "<p:sld {P}><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p>\
                 <a:p><a:r><a:t>Point</a:t></a:r><a:br/><a:r><a:t>more</a:t></a:r></a:p></p:txBody></p:sp>\
                 </p:spTree></p:cSld></p:sld>"
            )
        };
        let presentation = format!(
            "<p:presentation {P}><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId3\"/>\
             <p:sldId id=\"257\" r:id=\"rId2\"/></p:sldIdLst></p:presentation>"
        );
        let rels = "<Relationships><Relationship Id=\"rId1\" Type=\"x/slideMaster\" Target=\"slideMasters/m.xml\"/>\
            <Relationship Id=\"rId2\" Type=\"x/slide\" Target=\"slides/slide1.xml\"/>\
            <Relationship Id=\"rId3\" Type=\"x/slide\" Target=\"slides/slide2.xml\"/></Relationships>";
        let slide1_rels =
            "<Relationships><Relationship Id=\"rId1\" Type=\"http://x/relationships/notesSlide\" \
            Target=\"../notesSlides/notesSlide1.xml\"/></Relationships>";
        let notes = format!(
            "<p:notes {P}><p:cSld><p:spTree>\
             <p:sp><p:nvSpPr><p:nvPr><p:ph type=\"sldImg\"/></p:nvPr></p:nvSpPr></p:sp>\
             <p:sp><p:nvSpPr><p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr>\
             <p:txBody><a:p><a:r><a:t>Say this aloud</a:t></a:r></a:p></p:txBody></p:sp>\
             <p:sp><p:nvSpPr><p:nvPr><p:ph type=\"sldNum\"/></p:nvPr></p:nvSpPr>\
             <p:txBody><a:p><a:r><a:t>2</a:t></a:r></a:p></p:txBody></p:sp>\
             </p:spTree></p:cSld></p:notes>"
        );
        let (first, second) = (slide("Shown second"), slide("Shown first"));
        package(&[
            ("ppt/presentation.xml", presentation.as_str()),
            ("ppt/_rels/presentation.xml.rels", rels),
            ("ppt/slides/slide1.xml", first.as_str()),
            ("ppt/slides/slide2.xml", second.as_str()),
            ("ppt/slides/_rels/slide1.xml.rels", slide1_rels),
            ("ppt/notesSlides/notesSlide1.xml", notes.as_str()),
        ])
    }

    #[test]
    fn a_presentation_gives_its_slides_in_order_with_their_speaker_notes() {
        let read = import("talk.pptx", &presentation()).unwrap();
        assert_eq!(read.kind, FileKind::PowerPoint);
        // No title in its properties: the file name.
        assert_eq!(read.title, "talk");
        assert_eq!(read.pages, Some(2));
        assert_eq!(
            read.body,
            "Slide 1\nShown first\nPoint\nmore\n\nSlide 2\nShown second\nPoint\nmore\n\n\
             Speaker notes:\nSay this aloud"
        );
    }

    #[test]
    fn a_presentation_with_a_title_of_its_own_is_titled_by_it() {
        let mut parts = Vec::new();
        let bytes = presentation();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let mut content = String::new();
            entry.read_to_string(&mut content).unwrap();
            parts.push((entry.name().to_string(), content));
        }
        parts.push((
            "docProps/core.xml".to_string(),
            "<cp:coreProperties xmlns:cp=\"cp\" xmlns:dc=\"dc\"><dc:title>Cell division</dc:title>\
             </cp:coreProperties>"
                .to_string(),
        ));
        let parts: Vec<(&str, &str)> = parts
            .iter()
            .map(|(n, c)| (n.as_str(), c.as_str()))
            .collect();
        assert_eq!(
            import("talk.pptx", &package(&parts)).unwrap().title,
            "Cell division"
        );
    }

    #[test]
    fn relationship_targets_resolve_against_their_folder() {
        assert_eq!(resolve("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(
            resolve("ppt/slides", "../notesSlides/n1.xml"),
            "ppt/notesSlides/n1.xml"
        );
        assert_eq!(resolve("ppt/slides", "/ppt/media/a.png"), "ppt/media/a.png");
    }

    #[test]
    fn damaged_misnamed_and_empty_files_are_refused() {
        assert!(matches!(
            import("essay.docx", b"not a zip at all"),
            Err(ExtractError::Damaged(_))
        ));
        assert!(matches!(
            import(
                "talk.pptx",
                &package(&[("word/document.xml", "<w:document/>")])
            ),
            Err(ExtractError::Damaged(_))
        ));
        assert!(matches!(
            import("essay.docx", &word("<w:p><w:r><w:t>broken", None)),
            Err(ExtractError::Damaged(_))
        ));
        assert!(matches!(
            import("empty.txt", b" \n\t\n "),
            Err(ExtractError::NoText)
        ));
        assert!(matches!(
            import("empty.docx", &word("<w:p/><w:p/>", None)),
            Err(ExtractError::NoText)
        ));
    }

    #[test]
    fn files_over_the_limit_are_refused_before_being_read() {
        let folder =
            std::env::temp_dir().join(format!("synapse-extract-big-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("big.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(IMPORT_MAX_BYTES + 1).unwrap();
        let result = read_file(&path, Path::new("no-helper"));
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(matches!(result, Err(ExtractError::TooLarge)));
    }

    #[test]
    fn lines_a_pdf_broke_at_the_page_edge_are_joined_up() {
        let page = "Mitosis is the division of one cell's nucleus. It produces two genetically\n\
            identical daughter cells, each with the same number of chromo-\n\
            somes as the parent cell.\n\
            \n\
            The phases\n\
            1. Prophase: chromosomes condense and the spindle\n\
            starts to form.\n\
            2) Metaphase: they line up\n\
            - at the equator\n\
            Summary: two cells";
        assert_eq!(
            unwrap_lines(page),
            "Mitosis is the division of one cell's nucleus. It produces two genetically \
             identical daughter cells, each with the same number of chromo-somes as the parent cell.\n\
             \n\
             The phases\n\
             1. Prophase: chromosomes condense and the spindle starts to form.\n\
             2) Metaphase: they line up\n\
             - at the equator Summary: two cells"
        );
    }

    #[test]
    fn titles_fit_a_note() {
        assert_eq!(note_title("  Lecture\n 3 \t notes "), "Lecture 3 notes");
        assert_eq!(
            note_title(&"é".repeat(300)).chars().count(),
            NOTE_TITLE_MAX_CHARS
        );
        assert_eq!(note_title(" \n "), "Imported note");
    }

    #[test]
    fn a_pdf_helper_that_cannot_start_is_reported() {
        assert!(matches!(
            read_pdf(Path::new("/no/such/helper"), b"%PDF-1.4"),
            Err(ExtractError::HelperFailed(_))
        ));
    }
}
