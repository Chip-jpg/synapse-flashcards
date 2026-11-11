//! Lesson transcripts as note text.
//!
//! A lesson recorded and transcribed (for example by Dropscript, which writes
//! TXT, SRT, VTT and Markdown) becomes a note whose paragraphs each start with
//! a normalised time marker, `[m:ss]` or `[h:mm:ss]`, the way the note view
//! (`src/markdown.ts`) recognises as a timestamp.
//!
//! Three inputs are read, all by plain string handling, nothing else:
//! - SRT and WebVTT cue files (`from_cue_file`): the `WEBVTT` header, `NOTE`,
//!   `STYLE` and `REGION` blocks, cue identifiers and settings are dropped, and
//!   so are `<v>`, `<c>`, `<i>` and timestamp tags. A cue whose timing can't be
//!   read, or that has no text, is skipped.
//! - TXT and Markdown whose lines start `[hh:mm:ss]`, `[mm:ss]`,
//!   `[hh:mm:ss.mmm]` or `hh:mm:ss --> hh:mm:ss` (`from_timestamped_text`).
//!   Text without at least two such lines is none of these: `None`, and the
//!   caller keeps it unchanged.
//!
//! Consecutive short cues are merged into paragraphs of about half a minute.

/// A paragraph ends at the next cue once it is this old and ends a sentence.
const PARAGRAPH_SECONDS: u32 = 30;
/// A paragraph ends at the next cue once it is this old, sentence or not.
const PARAGRAPH_MAX_SECONDS: u32 = 60;
/// Fewer timestamped lines than this, and a text is not a transcript.
const MIN_TIMESTAMPED_LINES: usize = 2;

/// One caption or timestamped line: when it starts, in whole seconds, and its
/// text on one line.
struct Cue {
    start: u32,
    text: String,
}

enum Item {
    Cue(Cue),
    /// Text between timestamped lines (a heading, front matter), kept as is.
    Other(String),
}

/// The marker a paragraph starts with: `[m:ss]`, or `[h:mm:ss]` from an hour.
fn marker(seconds: u32) -> String {
    let hours = seconds / 3600;
    let minutes = seconds / 60 % 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("[{hours}:{minutes:02}:{secs:02}]")
    } else {
        format!("[{minutes}:{secs:02}]")
    }
}

fn digits(part: &str, max_len: usize) -> Option<u32> {
    if part.is_empty() || part.len() > max_len || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// A time as whole seconds: `h:mm:ss` or `m:ss`, with an optional `.mmm` or
/// `,mmm` after the seconds (dropped).
fn parse_time(text: &str) -> Option<u32> {
    let text = text.trim();
    let (whole, fraction) = match text.find(['.', ',']) {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    let parts: Vec<&str> = whole.split(':').collect();
    match parts.as_slice() {
        [minutes, seconds] => {
            let (minutes, seconds) = (digits(minutes, 3)?, digits(seconds, 2)?);
            (seconds < 60).then_some(minutes * 60 + seconds)
        }
        [hours, minutes, seconds] => {
            let (hours, minutes, seconds) =
                (digits(hours, 3)?, digits(minutes, 2)?, digits(seconds, 2)?);
            (minutes < 60 && seconds < 60).then_some(hours * 3600 + minutes * 60 + seconds)
        }
        _ => None,
    }
}

fn normalise_newlines(text: &str) -> String {
    text.trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The body of a note from the text of an SRT or WebVTT file; empty if the
/// file has no readable cue.
pub fn from_cue_file(text: &str) -> String {
    let text = normalise_newlines(text);
    let mut cues = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    // A final empty line closes the last block.
    for line in text.split('\n').chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if let Some(cue) = cue_of(&block) {
                cues.push(Item::Cue(cue));
            }
            block.clear();
        } else {
            block.push(line);
        }
    }
    paragraphs(cues)
}

/// The cue a block of lines holds, if it holds one.
fn cue_of(block: &[&str]) -> Option<Cue> {
    let first = block.first()?.trim();
    let header = first.starts_with("WEBVTT");
    let skipped = ["NOTE", "STYLE", "REGION"].iter().any(|word| {
        first
            .strip_prefix(*word)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    });
    if skipped {
        return None;
    }
    // The timing line is the first line, or the second after an index or an
    // identifier; a header block may carry metadata lines before it.
    let window = if header {
        block.len()
    } else {
        block.len().min(2)
    };
    let at = block[..window]
        .iter()
        .position(|line| line.contains("-->"))?;
    let start = cue_start(block[at])?;
    let lines: Vec<String> = block[at + 1..]
        .iter()
        .map(|line| strip_markup(line))
        .collect();
    let text = collapse_spaces(&lines.join(" "));
    (!text.is_empty()).then_some(Cue { start, text })
}

/// When a `start --> end [settings]` line starts, if both times read.
fn cue_start(line: &str) -> Option<u32> {
    let (left, right) = line.split_once("-->")?;
    let start = parse_time(left)?;
    parse_time(right.split_whitespace().next()?)?;
    Some(start)
}

/// Whether `rest` (what follows a `<`) starts a tag: a name, a closing slash
/// or a timestamp, so "a < b" stays text.
fn tag_start(rest: &str) -> bool {
    rest.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '/')
}

/// A cue line without `<v Name>`, `<c.class>`, `<i>`, `<00:00:01.000>` and
/// `{\an8}`, with the common character entities decoded.
fn strip_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find(['<', '{']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let (close, is_markup) = if let Some(after) = tail.strip_prefix('<') {
            ('>', tag_start(after))
        } else {
            ('}', tail[1..].starts_with('\\'))
        };
        if is_markup {
            if let Some(end) = tail.find(close) {
                rest = &tail[end + 1..];
                continue;
            }
        }
        // Not markup: the one-byte `<` or `{` is text.
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// The note body from a TXT or Markdown file with timestamped lines, or
/// `None` when it has too few to be a transcript (it is then left alone).
pub fn from_timestamped_text(text: &str) -> Option<String> {
    let text = normalise_newlines(text);
    let mut items: Vec<Item> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut cue_lines = 0;
    // Whether the last line belonged to a cue, so a line right after it,
    // without a blank line between, goes on with its text.
    let mut in_cue = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush_other(&mut other, &mut items);
            in_cue = false;
        } else if let Some((start, rest)) = leading_timestamp(trimmed) {
            flush_other(&mut other, &mut items);
            items.push(Item::Cue(Cue {
                start,
                text: collapse_spaces(rest),
            }));
            cue_lines += 1;
            in_cue = true;
        } else if in_cue && !trimmed.starts_with('#') {
            if let Some(Item::Cue(cue)) = items.last_mut() {
                if !cue.text.is_empty() {
                    cue.text.push(' ');
                }
                cue.text.push_str(&collapse_spaces(trimmed));
            }
        } else {
            in_cue = false;
            other.push(line.trim_end().to_string());
        }
    }
    flush_other(&mut other, &mut items);

    (cue_lines >= MIN_TIMESTAMPED_LINES).then(|| paragraphs(items))
}

fn flush_other(other: &mut Vec<String>, items: &mut Vec<Item>) {
    if !other.is_empty() {
        items.push(Item::Other(other.join("\n")));
        other.clear();
    }
}

/// The time a line starts with, and the text after it: `[h:mm:ss] text`,
/// `[m:ss.mmm] text`, `[00:01 --> 00:04] text` or `00:01 --> 00:04 text`.
fn leading_timestamp(line: &str) -> Option<(u32, &str)> {
    if let Some(inner) = line.strip_prefix('[') {
        let close = inner.find(']')?;
        let start = parse_time(inner[..close].split("-->").next()?)?;
        let rest = inner[close + 1..].trim();
        // `[1:30](address)` is a Markdown link, not a timestamp.
        if rest.starts_with('(') {
            return None;
        }
        return Some((start, rest));
    }
    if !line.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (left, right) = line.split_once("-->")?;
    let start = parse_time(left)?;
    let right = right.trim_start();
    let end = right.find(char::is_whitespace).unwrap_or(right.len());
    parse_time(&right[..end])?;
    Some((start, right[end..].trim()))
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', ')', '”', '’'])
        .ends_with(['.', '!', '?', '…', '。', '！', '？'])
}

/// The cues merged into paragraphs, each starting with its marker, separated
/// by blank lines; other text between them is kept in place.
fn paragraphs(items: Vec<Item>) -> String {
    struct Paragraph {
        start: u32,
        text: String,
    }
    fn flush(current: &mut Option<Paragraph>, out: &mut Vec<String>) {
        if let Some(paragraph) = current.take() {
            out.push(format!("{} {}", marker(paragraph.start), paragraph.text));
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut current: Option<Paragraph> = None;
    for item in items {
        match item {
            Item::Other(text) => {
                flush(&mut current, &mut out);
                out.push(text);
            }
            Item::Cue(cue) => {
                if cue.text.is_empty() {
                    continue;
                }
                let split = current.as_ref().is_some_and(|paragraph| {
                    let age = cue.start.saturating_sub(paragraph.start);
                    age >= PARAGRAPH_MAX_SECONDS
                        || (age >= PARAGRAPH_SECONDS && ends_sentence(&paragraph.text))
                });
                if split {
                    flush(&mut current, &mut out);
                }
                match current.as_mut() {
                    Some(paragraph) => {
                        paragraph.text.push(' ');
                        paragraph.text.push_str(&cue.text);
                    }
                    None => {
                        current = Some(Paragraph {
                            start: cue.start,
                            text: cue.text,
                        });
                    }
                }
            }
        }
    }
    flush(&mut current, &mut out);
    out.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_and_markers_are_normalised() {
        assert_eq!(parse_time("1:02:03"), Some(3723));
        assert_eq!(parse_time("00:00:01,500"), Some(1));
        assert_eq!(parse_time("12:34"), Some(754));
        assert_eq!(parse_time("0:05.250"), Some(5));
        assert_eq!(parse_time("00:61"), None);
        assert_eq!(parse_time("1"), None);
        assert_eq!(parse_time("ab:cd"), None);
        assert_eq!(parse_time("1:05."), None);
        assert_eq!(marker(59), "[0:59]");
        assert_eq!(marker(754), "[12:34]");
        assert_eq!(marker(3600), "[1:00:00]");
        assert_eq!(marker(3723), "[1:02:03]");
    }

    #[test]
    fn srt_cues_are_merged_into_paragraphs() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello there.\n\n\
                   2\n00:00:04,500 --> 00:00:07,000\nSecond line\nacross two.\n\n\
                   3\n00:00:35,000 --> 00:00:38,000\nNew paragraph.\n";
        assert_eq!(
            from_cue_file(srt),
            "[0:01] Hello there. Second line across two.\n\n[0:35] New paragraph."
        );
    }

    #[test]
    fn srt_with_crlf_and_a_byte_order_mark() {
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,000\r\nOne\r\n\r\n\
                   2\r\n00:00:03,000 --> 00:00:04,000\r\ntwo\r\n";
        assert_eq!(from_cue_file(srt), "[0:01] One two");
    }

    #[test]
    fn vtt_drops_header_notes_styles_settings_and_tags() {
        let vtt = "\u{feff}WEBVTT\nKind: captions\n\n\
                   NOTE a comment\nwith --> an arrow in it\n\n\
                   STYLE\n::cue { color: red }\n\n\
                   1\n00:01.000 --> 00:03.000 align:start position:0%\n\
                   <v Teacher>Hello <c.yellow>class</c> <00:00:02.000>today</c>\n\n\
                   01:00:05.250 --> 01:00:07.000\nAfter an hour &amp; more\n";
        assert_eq!(
            from_cue_file(vtt),
            "[0:01] Hello class today\n\n[1:00:05] After an hour & more"
        );
    }

    #[test]
    fn a_vtt_header_without_a_blank_line_keeps_its_first_cue() {
        let vtt = "WEBVTT\n00:00:02.000 --> 00:00:03.000\nStraight in\n";
        assert_eq!(from_cue_file(vtt), "[0:02] Straight in");
    }

    #[test]
    fn malformed_cues_are_skipped() {
        let srt = "1\nbad --> worse\ntext\n\n\
                   2\n00:00:02,000 --> 00:00:03,000\nGood\n\n\
                   3\n00:00:03,000 --> 00:00:04,000\n\n\
                   4\nno timing at all\n";
        assert_eq!(from_cue_file(srt), "[0:02] Good");
        assert_eq!(from_cue_file("just some words"), "");
        assert_eq!(from_cue_file(""), "");
    }

    #[test]
    fn text_that_is_not_a_cue_isnt_stripped_of_angle_brackets() {
        assert_eq!(strip_markup("if a < b and c > d"), "if a < b and c > d");
        assert_eq!(strip_markup("{\\an8}Up top"), "Up top");
        assert_eq!(strip_markup("a {b} c"), "a {b} c");
        assert_eq!(strip_markup("unclosed <i text"), "unclosed <i text");
    }

    #[test]
    fn dropscript_text_is_normalised_and_merged() {
        let txt = "[00:00:01] Hello everyone.\n[00:00:05] Today we talk.\n\
                   [00:00:40] Next part.\n[01:02:03.500] Late.\n";
        assert_eq!(
            from_timestamped_text(txt).unwrap(),
            "[0:01] Hello everyone. Today we talk.\n\n[0:40] Next part.\n\n[1:02:03] Late."
        );
    }

    #[test]
    fn short_markers_crlf_and_a_byte_order_mark() {
        let txt = "\u{feff}[0:05] One\r\n[0:09] two\r\n";
        assert_eq!(from_timestamped_text(txt).unwrap(), "[0:05] One two");
    }

    #[test]
    fn range_style_lines_are_read() {
        let txt = "00:00:01.000 --> 00:00:04.000 Hello\n[00:04 --> 00:06] world\n";
        assert_eq!(from_timestamped_text(txt).unwrap(), "[0:01] Hello world");
    }

    #[test]
    fn a_line_after_a_cue_goes_on_with_its_text() {
        let txt = "[00:01] Start\ncontinues here\n[00:03] more\n";
        assert_eq!(
            from_timestamped_text(txt).unwrap(),
            "[0:01] Start continues here more"
        );
    }

    #[test]
    fn markdown_headings_stay_in_place() {
        let md =
            "# Lesson\n\n[00:00:01] Intro.\n[00:00:10] More.\n\n## Part two\n\n[00:00:50] Later.\n";
        assert_eq!(
            from_timestamped_text(md).unwrap(),
            "# Lesson\n\n[0:01] Intro. More.\n\n## Part two\n\n[0:50] Later."
        );
    }

    #[test]
    fn a_paragraph_is_cut_after_a_minute_even_without_a_sentence_end() {
        let txt: String = (0..7)
            .map(|n| format!("[{}:{:02}] w{n}\n", n * 10 / 60, n * 10 % 60))
            .collect();
        assert_eq!(
            from_timestamped_text(&txt).unwrap(),
            "[0:00] w0 w1 w2 w3 w4 w5\n\n[1:00] w6"
        );
    }

    #[test]
    fn text_without_timestamps_is_left_alone() {
        assert_eq!(from_timestamped_text("Plain notes\nwith lines\n"), None);
        // Brackets that aren't times, and one lone timestamp, are not a transcript.
        assert_eq!(
            from_timestamped_text("See [1] and [2]\n[1] A footnote\n[12:30] once\n"),
            None
        );
        assert_eq!(from_timestamped_text("[1:30](https://example.org)\n"), None);
        assert_eq!(from_timestamped_text(""), None);
    }
}
