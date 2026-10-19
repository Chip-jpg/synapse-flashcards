//! Cards in spreadsheet files: importing front/back cards from a CSV or TSV
//! file into a deck, and exporting a deck's cards as CSV.
//!
//! This is not a backup and not a merge of one: only a card's front and back
//! travel. An imported card starts as a brand-new card, exactly like one the
//! user typed, and an exported file holds no schedule or review history.
//!
//! Importing has two steps, so nothing is saved before the user has seen it:
//! 1. [`read_cards`] reads the file and checks every row against the deck,
//!    writing nothing. Each row is checked as a typed card is
//!    (`authoring::validate_card`), and a row whose front is already in the
//!    deck, or on an earlier row of the file, is marked as a duplicate.
//! 2. [`import_cards`] saves the rows the user kept, in one transaction:
//!    every card is checked again, and if any is refused, or the deck can't
//!    take cards, none is saved. Duplicates are found again at that moment
//!    (the deck may have changed since step 1) and left out if asked.
//!
//! Reading a file:
//! - Its text is UTF-8 (with or without a byte order mark), UTF-16 with one,
//!   or else Windows-1252, read as `extract.rs` reads a text file.
//! - The delimiter is a tab for a `.tsv` file, and otherwise whichever of
//!   comma, semicolon, or tab appears most often (outside quotes) in the first
//!   line; a comma when none does.
//! - Fields follow RFC 4180: a field that starts with `"` runs to the next
//!   lone `"`, may hold delimiters and line breaks, and writes a `"` as `""`.
//!   Lines may end with CRLF, LF, or CR. Blank lines are skipped.
//! - The first column is the front and the second the back; further columns
//!   are ignored (and counted, so the user is told).
//! - The first row is a header, and not a card, when its first two cells are
//!   column names such as "Front" and "Back" ([`HEADER_FRONTS`],
//!   [`HEADER_BACKS`]).
//!
//! Exporting writes RFC 4180 CSV: a `front,back` header, CRLF line ends, and
//! fields quoted when they hold a comma, quote, or line break. It starts with
//! a UTF-8 byte order mark, so Excel reads accented letters correctly; Synapse
//! reads it back the same way.
//!
//! Nothing here logs a card's text.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::authoring::{
    insert_new_card, load_deck_detail, validate_card, AuthoringError, CardText, InvalidInput,
};
use crate::db::to_db_time;
use crate::extract::decode_text;

/// The largest file read: far more than [`CSV_ROWS_MAX`] cards of ordinary
/// length, and small enough to read at once.
pub const CSV_FILE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// The most rows (cards) one file may hold, not counting a header, and the
/// most cards [`import_cards`] saves at once.
pub const CSV_ROWS_MAX: usize = 5_000;

/// Header names for the front column, compared ignoring case and outer spaces.
pub const HEADER_FRONTS: [&str; 6] = ["front", "question", "term", "prompt", "fronte", "domanda"];
/// Header names for the back column.
pub const HEADER_BACKS: [&str; 6] = [
    "back",
    "answer",
    "definition",
    "response",
    "retro",
    "risposta",
];

/// The character between fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
}

impl Delimiter {
    fn char(self) -> char {
        match self {
            Delimiter::Comma => ',',
            Delimiter::Semicolon => ';',
            Delimiter::Tab => '\t',
        }
    }
}

/// Why a row can't become a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowProblem {
    /// The row has only one column, so no back.
    NoBack,
    /// The row's front or back fails the rules of a typed card.
    Card(InvalidInput),
}

/// Why a row's card may not be wanted: its front is already in the deck, or
/// on an earlier row of this file (by its line number). Sent as
/// `{ "kind": "inDeck" }` or `{ "kind": "inFile", "line": 3 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Duplicate {
    InDeck,
    InFile { line: usize },
}

/// One row of the file, checked. `front` and `back` are as the file has them,
/// trimmed of outer whitespace.
#[derive(Debug, Clone, PartialEq)]
pub struct CsvRow {
    /// The line of the file the row starts on, counting from 1.
    pub line: usize,
    pub front: String,
    pub back: String,
    pub problem: Option<RowProblem>,
    pub duplicate: Option<Duplicate>,
}

/// A whole file, checked against a deck. Nothing has been saved.
#[derive(Debug, PartialEq)]
pub struct CsvPreview {
    pub delimiter: Delimiter,
    /// The first row was column names, so it isn't in `rows`.
    pub header: bool,
    pub rows: Vec<CsvRow>,
    /// Rows with something in a third or later column, which is ignored.
    pub rows_with_extra_columns: usize,
}

/// What saving imported cards did.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardsImported {
    /// Cards added to the deck.
    pub added: usize,
    /// Cards left out as duplicates.
    pub skipped: usize,
    /// The deck's active cards afterwards.
    pub card_count: i64,
}

/// Why a file couldn't be read, or its cards couldn't be saved. Nothing was
/// written.
#[derive(Debug)]
pub enum CsvError {
    /// The file is larger than [`CSV_FILE_MAX_BYTES`].
    TooLarge,
    /// The file has more than [`CSV_ROWS_MAX`] rows, or that many cards were
    /// sent to save.
    TooManyRows,
    /// The file has no rows (or only a header).
    NoRows,
    /// A quoted field starting on this line never ends.
    UnclosedQuote { line: usize },
    /// Every card sent was a duplicate, so there was nothing to add.
    OnlyDuplicates,
    /// The deck refused, or a card failed its checks (see `create_flashcards`).
    Authoring(AuthoringError),
}

impl From<AuthoringError> for CsvError {
    fn from(err: AuthoringError) -> Self {
        CsvError::Authoring(err)
    }
}

impl From<sqlx::Error> for CsvError {
    fn from(err: sqlx::Error) -> Self {
        CsvError::Authoring(err.into())
    }
}

/// A record of the file: its fields, and the line it starts on.
type Record = (usize, Vec<String>);

/// The delimiter for a file: tab for `.tsv`, else the most common candidate
/// outside quotes in the first non-blank line (ties go comma, semicolon, tab).
fn detect_delimiter(text: &str, tsv: bool) -> Delimiter {
    if tsv {
        return Delimiter::Tab;
    }
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let mut counts = [
        (Delimiter::Comma, 0),
        (Delimiter::Semicolon, 0),
        (Delimiter::Tab, 0),
    ];
    let mut quoted = false;
    for ch in first.chars() {
        if ch == '"' {
            quoted = !quoted;
        } else if !quoted {
            for (delimiter, count) in &mut counts {
                if ch == delimiter.char() {
                    *count += 1;
                }
            }
        }
    }
    // `max_by_key` keeps the last of equal maxima, so search from the back to
    // let the earlier candidate win a tie.
    counts
        .iter()
        .rev()
        .max_by_key(|(_, count)| *count)
        .map(|(delimiter, _)| *delimiter)
        .unwrap_or(Delimiter::Comma)
}

/// Splits text into records by RFC 4180, leniently: a quote in the middle of
/// an unquoted field is kept as written, as is text after a closing quote.
/// Blank lines give no record.
fn parse_records(text: &str, delimiter: Delimiter) -> Result<Vec<Record>, CsvError> {
    let delimiter = delimiter.char();
    let mut records = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut line = 1;
    let mut record_line = 1;
    // Whether the current field opened with a quote and it hasn't closed.
    let mut quoted = false;
    // Whether anything (a character or a quote) has been read into the field.
    let mut started = false;
    let mut chars = text.chars().peekable();

    let end_record = |fields: &mut Vec<String>, records: &mut Vec<Record>, record_line| {
        let record = std::mem::take(fields);
        let blank = record.len() == 1 && record[0].trim().is_empty();
        if !blank {
            records.push((record_line, record));
        }
    };

    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\r' | '\n' => {
                    if ch == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    // Line breaks inside a field are kept as `\n`.
                    field.push('\n');
                    line += 1;
                }
                _ => field.push(ch),
            }
            continue;
        }
        match ch {
            '"' if !started => {
                quoted = true;
                started = true;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                fields.push(std::mem::take(&mut field));
                end_record(&mut fields, &mut records, record_line);
                started = false;
                line += 1;
                record_line = line;
            }
            _ if ch == delimiter => {
                fields.push(std::mem::take(&mut field));
                started = false;
            }
            _ => {
                field.push(ch);
                started = true;
            }
        }
    }
    if quoted {
        return Err(CsvError::UnclosedQuote { line: record_line });
    }
    if started || !fields.is_empty() {
        fields.push(field);
        end_record(&mut fields, &mut records, record_line);
    }
    Ok(records)
}

/// Whether a record's first two cells are column names, not a card.
fn is_header(fields: &[String]) -> bool {
    let name = |index: usize| fields.get(index).map(|cell| cell.trim().to_lowercase());
    matches!(
        (name(0), name(1)),
        (Some(front), Some(back))
            if HEADER_FRONTS.contains(&front.as_str()) && HEADER_BACKS.contains(&back.as_str())
    )
}

/// Reads a CSV or TSV file's rows and checks each one as a card for `deck_id`,
/// which must be an active normal deck. Writes nothing.
///
/// `tsv` says the file's name ends in `.tsv`, so its delimiter is a tab.
pub async fn read_cards(
    pool: &SqlitePool,
    deck_id: i64,
    bytes: &[u8],
    tsv: bool,
    now: DateTime<Utc>,
) -> Result<CsvPreview, CsvError> {
    if bytes.len() as u64 > CSV_FILE_MAX_BYTES {
        return Err(CsvError::TooLarge);
    }
    let text = decode_text(bytes);
    let delimiter = detect_delimiter(&text, tsv);
    let mut records = parse_records(&text, delimiter)?;
    let header = records.first().is_some_and(|(_, fields)| is_header(fields));
    if header {
        records.remove(0);
    }
    if records.is_empty() {
        return Err(CsvError::NoRows);
    }
    if records.len() > CSV_ROWS_MAX {
        return Err(CsvError::TooManyRows);
    }

    // Refuses a missing, archived, or sample deck before anything else.
    let mut conn = pool.acquire().await?;
    let deck = load_deck_detail(&mut conn, deck_id, &to_db_time(now)).await?;
    let in_deck: HashSet<String> = deck
        .cards
        .into_iter()
        .map(|card| card.front.trim().to_string())
        .collect();

    let mut rows_with_extra_columns = 0;
    // The line each front was first seen on, for duplicates within the file.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let rows = records
        .into_iter()
        .map(|(line, fields)| {
            if fields.iter().skip(2).any(|cell| !cell.trim().is_empty()) {
                rows_with_extra_columns += 1;
            }
            let front = fields[0].trim().to_string();
            let back = fields.get(1).map(|cell| cell.trim().to_string());
            let problem = match &back {
                None => Some(RowProblem::NoBack),
                Some(back) => validate_card(&front, back).err().map(RowProblem::Card),
            };
            let duplicate = if front.is_empty() {
                None
            } else if in_deck.contains(&front) {
                Some(Duplicate::InDeck)
            } else if let Some(&first) = seen.get(&front) {
                Some(Duplicate::InFile { line: first })
            } else {
                // Only a row that can become a card makes later ones repeats.
                if problem.is_none() {
                    seen.insert(front.clone(), line);
                }
                None
            };
            CsvRow {
                line,
                front,
                back: back.unwrap_or_default(),
                problem,
                duplicate,
            }
        })
        .collect();

    Ok(CsvPreview {
        delimiter,
        header,
        rows,
        rows_with_extra_columns,
    })
}

/// Adds imported cards to an active normal deck, in the order given, each
/// checked and started exactly as a typed card is. All or nothing, in one
/// transaction, as `authoring::create_flashcards`.
///
/// With `skip_duplicates`, a card whose front is already in the deck, or on
/// an earlier card sent, is left out (and counted). If that leaves nothing to
/// add, nothing is written and [`CsvError::OnlyDuplicates`] is returned.
pub async fn import_cards(
    pool: &SqlitePool,
    deck_id: i64,
    cards: &[CardText],
    skip_duplicates: bool,
    now: DateTime<Utc>,
) -> Result<CardsImported, CsvError> {
    if cards.is_empty() {
        return Err(AuthoringError::NoCards.into());
    }
    if cards.len() > CSV_ROWS_MAX {
        return Err(CsvError::TooManyRows);
    }
    let cards = cards
        .iter()
        .enumerate()
        .map(|(index, card)| {
            validate_card(&card.front, &card.back)
                .map_err(|problem| AuthoringError::InvalidCard(index, problem))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // Refused here, before any card is written.
    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    let mut fronts: HashSet<String> = deck.cards.into_iter().map(|card| card.front).collect();

    let mut added = 0;
    let mut skipped = 0;
    for card in &cards {
        // Stored fronts are already trimmed, as `validate_card` trims these.
        if skip_duplicates && fronts.contains(&card.front) {
            skipped += 1;
            continue;
        }
        insert_new_card(&mut tx, deck_id, card, &now).await?;
        fronts.insert(card.front.clone());
        added += 1;
    }
    if added == 0 {
        // Dropping `tx` rolls it back, though nothing was written.
        return Err(CsvError::OnlyDuplicates);
    }

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(CardsImported {
        added,
        skipped,
        card_count: deck.card_count,
    })
}

/// A deck's name and its active cards, oldest first, for exporting. Any
/// active deck can be exported, the sample deck included, since nothing
/// changes; an archived deck is refused like every other use of it.
pub async fn deck_cards(
    pool: &SqlitePool,
    deck_id: i64,
) -> Result<(String, Vec<CardText>), AuthoringError> {
    let deck: Option<(String, bool)> =
        sqlx::query_as("SELECT name, archived_at IS NOT NULL FROM decks WHERE id = ?1")
            .bind(deck_id)
            .fetch_optional(pool)
            .await?;
    let name = match deck {
        None => return Err(AuthoringError::DeckNotFound),
        Some((_, true)) => return Err(AuthoringError::DeckArchived),
        Some((name, false)) => name,
    };
    let cards: Vec<(String, String)> = sqlx::query_as(
        "SELECT front, back FROM flashcards
         WHERE deck_id = ?1 AND deleted_at IS NULL
         ORDER BY id",
    )
    .bind(deck_id)
    .fetch_all(pool)
    .await?;
    if cards.is_empty() {
        return Err(AuthoringError::NoCards);
    }
    let cards = cards
        .into_iter()
        .map(|(front, back)| CardText { front, back })
        .collect();
    Ok((name, cards))
}

/// One field as RFC 4180 writes it: quoted (with quotes doubled) when it
/// holds a comma, quote, or line break, and as it is otherwise.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// Cards as an RFC 4180 CSV file, header first, with a UTF-8 byte order mark.
/// Line breaks inside a card are written as CRLF, as the format's own are.
pub fn write_csv(cards: &[CardText]) -> String {
    let mut out = String::from("\u{FEFF}front,back\r\n");
    for card in cards {
        let field = |text: &str| csv_field(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
        out.push_str(&field(&card.front));
        out.push(',');
        out.push_str(&field(&card.back));
        out.push_str("\r\n");
    }
    out
}

/// A file name for a deck's export: its name with the characters Windows
/// doesn't allow in file names replaced, then `.csv`.
pub fn suggested_file_name(deck_name: &str) -> String {
    let name: String = deck_name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '-'
            } else {
                ch
            }
        })
        .collect();
    let name = name.trim().trim_end_matches('.').trim();
    if name.is_empty() {
        "cards.csv".to_string()
    } else {
        format!("{name}.csv")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{archive_deck, create_deck, create_flashcard, deck_detail};
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;

    const NOW: &str = "2026-10-19T12:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    async fn deck_with(pool: &SqlitePool, name: &str, cards: &[(&str, &str)]) -> i64 {
        let deck = create_deck(pool, name, None, at(NOW)).await.unwrap().id;
        for (front, back) in cards {
            create_flashcard(pool, deck, front, back, at(NOW))
                .await
                .unwrap();
        }
        deck
    }

    fn card(front: &str, back: &str) -> CardText {
        CardText {
            front: front.to_string(),
            back: back.to_string(),
        }
    }

    fn records(text: &str, delimiter: Delimiter) -> Vec<Record> {
        parse_records(text, delimiter).unwrap()
    }

    fn fields(text: &str, delimiter: Delimiter) -> Vec<Vec<String>> {
        records(text, delimiter)
            .into_iter()
            .map(|(_, fields)| fields)
            .collect()
    }

    async fn read(pool: &SqlitePool, deck: i64, bytes: &[u8]) -> CsvPreview {
        read_cards(pool, deck, bytes, false, at(NOW)).await.unwrap()
    }

    #[test]
    fn the_delimiter_is_the_commonest_one_in_the_first_line() {
        assert_eq!(detect_delimiter("a,b\nc;d;e", false), Delimiter::Comma);
        assert_eq!(detect_delimiter("a;b;c\n", false), Delimiter::Semicolon);
        assert_eq!(detect_delimiter("a\tb\n", false), Delimiter::Tab);
        // Inside quotes doesn't count.
        assert_eq!(
            detect_delimiter("\"a,b,c\";d\n", false),
            Delimiter::Semicolon
        );
        // Blank lines first are passed over; nothing at all is a comma.
        assert_eq!(detect_delimiter("\n\na;b", false), Delimiter::Semicolon);
        assert_eq!(detect_delimiter("just one column", false), Delimiter::Comma);
        // A tie goes to the comma.
        assert_eq!(detect_delimiter("a,b;c", false), Delimiter::Comma);
        // A .tsv file is tab-separated whatever its first line has.
        assert_eq!(detect_delimiter("a,b,c", true), Delimiter::Tab);
    }

    #[test]
    fn fields_follow_rfc_4180() {
        let text = "front,back\r\n\"Hello, world\",\"Say \"\"hi\"\"\"\r\n\"Two\r\nlines\",x\n";
        assert_eq!(
            fields(text, Delimiter::Comma),
            vec![
                vec!["front", "back"],
                vec!["Hello, world", "Say \"hi\""],
                vec!["Two\nlines", "x"],
            ]
        );
    }

    #[test]
    fn records_know_the_line_they_start_on_and_skip_blank_lines() {
        let text = "a,b\n\n\"multi\nline\",c\r\rd,e";
        let lines: Vec<usize> = records(text, Delimiter::Comma)
            .into_iter()
            .map(|(line, _)| line)
            .collect();
        assert_eq!(lines, vec![1, 3, 6]);
    }

    #[test]
    fn stray_quotes_are_kept_and_an_unclosed_one_is_refused() {
        assert_eq!(
            fields("He said \"no\",b\n\"q\"x,y", Delimiter::Comma),
            vec![vec!["He said \"no\"", "b"], vec!["qx", "y"]]
        );
        assert!(matches!(
            parse_records("a,b\nc,\"never\nends", Delimiter::Comma),
            Err(CsvError::UnclosedQuote { line: 2 })
        ));
    }

    #[test]
    fn empty_trailing_fields_and_missing_final_line_break_are_kept() {
        assert_eq!(
            fields("a,\nb,c", Delimiter::Comma),
            vec![vec!["a", ""], vec!["b", "c"]]
        );
        assert_eq!(
            fields("a;b;", Delimiter::Semicolon),
            vec![vec!["a", "b", ""]]
        );
    }

    #[test]
    fn a_header_is_recognised_by_its_column_names() {
        let row = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
        assert!(is_header(&row("Front", "Back")));
        assert!(is_header(&row(" question ", "ANSWER")));
        assert!(is_header(&row("Domanda", "Risposta")));
        assert!(!is_header(&row("Front", "The back of a card")));
        assert!(!is_header(&["front".to_string()]));
    }

    #[test]
    fn rows_are_checked_as_typed_cards_with_duplicates_marked() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Biology", &[("What is a cell?", "A unit of life.")]).await;
            let too_long = "x".repeat(2_001);
            let file = format!(
                "Front,Back,Notes\n\
                 What is a cell?,Again\n\
                 What is DNA?,A molecule.,from chapter 2\n\
                 Only a front\n\
                 ,No front\n\
                 Long,{too_long}\n\
                 \" What is DNA? \",Repeated\n"
            );

            let preview = read(&pool, deck, file.as_bytes()).await;

            assert_eq!(preview.delimiter, Delimiter::Comma);
            assert!(preview.header);
            assert_eq!(preview.rows_with_extra_columns, 1);
            let summary: Vec<_> = preview
                .rows
                .iter()
                .map(|row| (row.line, row.front.as_str(), row.problem, row.duplicate))
                .collect();
            assert_eq!(
                summary,
                vec![
                    (2, "What is a cell?", None, Some(Duplicate::InDeck)),
                    (3, "What is DNA?", None, None),
                    (4, "Only a front", Some(RowProblem::NoBack), None),
                    (
                        5,
                        "",
                        Some(RowProblem::Card(InvalidInput::FrontBlank)),
                        None
                    ),
                    (
                        6,
                        "Long",
                        Some(RowProblem::Card(InvalidInput::BackTooLong)),
                        None
                    ),
                    (7, "What is DNA?", None, Some(Duplicate::InFile { line: 3 })),
                ]
            );
            // Reading wrote nothing.
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 2);
        });
    }

    #[test]
    fn files_are_read_in_any_encoding_extract_reads() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "French", &[]).await;

            let mut bom = vec![0xEF, 0xBB, 0xBF];
            bom.extend_from_slice("Café;Coffee\n".as_bytes());
            let preview = read(&pool, deck, &bom).await;
            assert_eq!(preview.delimiter, Delimiter::Semicolon);
            assert_eq!(preview.rows[0].front, "Café");

            // Windows-1252, as Excel saves "CSV" on many Windows computers.
            let preview = read(&pool, deck, b"Caf\xe9,\x93Coffee\x94\n").await;
            assert_eq!(
                (
                    preview.rows[0].front.as_str(),
                    preview.rows[0].back.as_str()
                ),
                ("Café", "“Coffee”")
            );

            let preview = read_cards(&pool, deck, b"one,two\tthree", true, at(NOW))
                .await
                .unwrap();
            assert_eq!(preview.delimiter, Delimiter::Tab);
            assert_eq!(preview.rows[0].front, "one,two");
        });
    }

    #[test]
    fn unusable_files_and_decks_are_refused() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Biology", &[]).await;

            let empty = read_cards(&pool, deck, b"\n\n", false, at(NOW)).await;
            assert!(matches!(empty, Err(CsvError::NoRows)));
            let header_only = read_cards(&pool, deck, b"front,back\n", false, at(NOW)).await;
            assert!(matches!(header_only, Err(CsvError::NoRows)));

            let many = "q,a\n".repeat(CSV_ROWS_MAX + 1);
            let too_many = read_cards(&pool, deck, many.as_bytes(), false, at(NOW)).await;
            assert!(matches!(too_many, Err(CsvError::TooManyRows)));
            let most = "q,a\n".repeat(CSV_ROWS_MAX);
            let preview = read(&pool, deck, most.as_bytes()).await;
            assert_eq!(preview.rows.len(), CSV_ROWS_MAX);

            let huge = vec![b'a'; CSV_FILE_MAX_BYTES as usize + 1];
            let too_large = read_cards(&pool, deck, &huge, false, at(NOW)).await;
            assert!(matches!(too_large, Err(CsvError::TooLarge)));

            let sample = sample_deck_id(&pool).await;
            let refused = read_cards(&pool, sample, b"a,b", false, at(NOW)).await;
            assert!(matches!(
                refused,
                Err(CsvError::Authoring(AuthoringError::SampleDeck))
            ));
            let missing = read_cards(&pool, 999, b"a,b", false, at(NOW)).await;
            assert!(matches!(
                missing,
                Err(CsvError::Authoring(AuthoringError::DeckNotFound))
            ));
        });
    }

    #[test]
    fn imported_cards_are_new_cards_saved_all_together() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Biology", &[("What is a cell?", "A unit.")]).await;

            let imported = import_cards(
                &pool,
                deck,
                &[
                    card(" What is DNA? ", "A molecule.\nIn cells."),
                    card("What is RNA?", "Another one."),
                ],
                true,
                at(NOW),
            )
            .await
            .unwrap();

            assert_eq!(
                imported,
                CardsImported {
                    added: 2,
                    skipped: 0,
                    card_count: 3
                }
            );
            let stored: Vec<(String, String, String, Option<String>, i64)> = sqlx::query_as(
                "SELECT front, back, fsrs_state, due, reps FROM flashcards
                 WHERE deck_id = ?1 ORDER BY id",
            )
            .bind(deck)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                stored[1..],
                [
                    (
                        "What is DNA?".to_string(),
                        "A molecule.\nIn cells.".to_string(),
                        "New".to_string(),
                        None,
                        0
                    ),
                    (
                        "What is RNA?".to_string(),
                        "Another one.".to_string(),
                        "New".to_string(),
                        None,
                        0
                    ),
                ]
            );
        });
    }

    #[test]
    fn duplicates_are_skipped_only_when_asked() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Biology", &[("What is a cell?", "A unit.")]).await;
            let cards = [
                card("What is a cell?", "Again"),
                card("What is DNA?", "A molecule."),
                card("What is DNA?", "Repeated."),
            ];

            let skipped = import_cards(&pool, deck, &cards, true, at(NOW))
                .await
                .unwrap();
            assert_eq!((skipped.added, skipped.skipped), (1, 2));

            let kept = import_cards(&pool, deck, &cards, false, at(NOW))
                .await
                .unwrap();
            assert_eq!((kept.added, kept.skipped, kept.card_count), (3, 0, 5));

            // Nothing new at all is refused rather than "added 0".
            let only = import_cards(&pool, deck, &cards[..1], true, at(NOW)).await;
            assert!(matches!(only, Err(CsvError::OnlyDuplicates)));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 6);
        });
    }

    #[test]
    fn a_refused_card_or_deck_saves_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Biology", &[]).await;
            let before = count(&pool, "SELECT COUNT(*) FROM flashcards").await;

            let bad = import_cards(
                &pool,
                deck,
                &[card("Fine", "Fine"), card("No back", "  ")],
                true,
                at(NOW),
            )
            .await;
            assert!(matches!(
                bad,
                Err(CsvError::Authoring(AuthoringError::InvalidCard(
                    1,
                    InvalidInput::BackBlank
                )))
            ));
            let none = import_cards(&pool, deck, &[], true, at(NOW)).await;
            assert!(matches!(
                none,
                Err(CsvError::Authoring(AuthoringError::NoCards))
            ));
            let many = vec![card("q", "a"); CSV_ROWS_MAX + 1];
            let too_many = import_cards(&pool, deck, &many, false, at(NOW)).await;
            assert!(matches!(too_many, Err(CsvError::TooManyRows)));

            let sample = sample_deck_id(&pool).await;
            let refused = import_cards(&pool, sample, &[card("q", "a")], true, at(NOW)).await;
            assert!(matches!(
                refused,
                Err(CsvError::Authoring(AuthoringError::SampleDeck))
            ));
            archive_deck(&pool, deck, at(NOW)).await.unwrap();
            let archived = import_cards(&pool, deck, &[card("q", "a")], true, at(NOW)).await;
            assert!(matches!(
                archived,
                Err(CsvError::Authoring(AuthoringError::DeckArchived))
            ));

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM flashcards").await,
                before
            );
        });
    }

    #[test]
    fn many_cards_are_saved_in_one_go() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Big", &[]).await;
            let cards: Vec<CardText> = (0..CSV_ROWS_MAX)
                .map(|n| card(&format!("Q{n}"), "A"))
                .collect();
            let imported = import_cards(&pool, deck, &cards, true, at(NOW))
                .await
                .unwrap();
            assert_eq!(imported.added, CSV_ROWS_MAX);
            let detail = deck_detail(&pool, deck, at(NOW)).await.unwrap();
            assert_eq!(detail.card_count, CSV_ROWS_MAX as i64);
        });
    }

    #[test]
    fn export_writes_rfc_4180_that_reads_back_the_same() {
        let cards = vec![
            card("Plain", "Text"),
            card("Hello, world", "Say \"hi\""),
            card("Two\nlines", "Café"),
        ];
        let csv = write_csv(&cards);
        assert_eq!(
            csv,
            "\u{FEFF}front,back\r\nPlain,Text\r\n\"Hello, world\",\"Say \"\"hi\"\"\"\r\n\
             \"Two\r\nlines\",Café\r\n"
        );

        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Round trip", &[]).await;
            let preview = read(&pool, deck, csv.as_bytes()).await;
            assert!(preview.header);
            let back: Vec<CardText> = preview
                .rows
                .into_iter()
                .map(|row| card(&row.front, &row.back))
                .collect();
            assert_eq!(back, cards);
        });
    }

    #[test]
    fn a_deck_exports_its_active_cards_in_order() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_with(&pool, "Biology", &[("One", "1"), ("Two", "2")]).await;
            let two: i64 = sqlx::query_scalar("SELECT id FROM flashcards WHERE front = 'Two'")
                .fetch_one(&pool)
                .await
                .unwrap();
            crate::authoring::delete_flashcard(&pool, two, at(NOW))
                .await
                .unwrap();

            let (name, cards) = deck_cards(&pool, deck).await.unwrap();
            assert_eq!(name, "Biology");
            assert_eq!(cards, vec![card("One", "1")]);

            // The sample deck can be exported: nothing changes.
            let sample = sample_deck_id(&pool).await;
            assert!(!deck_cards(&pool, sample).await.unwrap().1.is_empty());

            let empty = deck_with(&pool, "Empty", &[]).await;
            assert!(matches!(
                deck_cards(&pool, empty).await,
                Err(AuthoringError::NoCards)
            ));
            archive_deck(&pool, deck, at(NOW)).await.unwrap();
            assert!(matches!(
                deck_cards(&pool, deck).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert!(matches!(
                deck_cards(&pool, 999).await,
                Err(AuthoringError::DeckNotFound)
            ));
        });
    }

    #[test]
    fn export_file_names_are_safe_on_windows() {
        assert_eq!(suggested_file_name("Biology"), "Biology.csv");
        assert_eq!(suggested_file_name("A/B: c?"), "A-B- c-.csv");
        assert_eq!(suggested_file_name("Notes..."), "Notes.csv");
        assert_eq!(suggested_file_name(" ... "), "cards.csv");
    }
}
