//! Reading a PDF through the helper process (see `extract.rs`): Synapse,
//! started by itself to read the text. These run the real Synapse binary as
//! the helper, which the unit tests can't.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use synapse_scaffold_lib::extract::{self, ExtractError, ExtractedText, FileKind};

const HELPER: &str = env!("CARGO_BIN_EXE_synapse-scaffold");

/// A PDF with one page per entry of `pages`, each line of an entry a line of
/// text on that page. An empty entry is a page with no text.
fn pdf(pages: &[&str]) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let mut kids: Vec<Object> = Vec::new();
    for page in pages {
        let mut operations = Vec::new();
        if !page.is_empty() {
            operations.push(Operation::new("BT", vec![]));
            operations.push(Operation::new("Tf", vec!["F1".into(), 14.into()]));
            operations.push(Operation::new("Td", vec![72.into(), 760.into()]));
            for (number, line) in page.lines().enumerate() {
                if number > 0 {
                    operations.push(Operation::new("Td", vec![0.into(), (-20).into()]));
                }
                operations.push(Operation::new("Tj", vec![Object::string_literal(line)]));
            }
            operations.push(Operation::new("ET", vec![]));
        }
        let content = Content { operations }.encode().unwrap();
        let content_id = doc.add_object(Stream::new(dictionary! {}, content));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        kids.push(page_id.into());
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

/// Writes `bytes` to `name` in a folder of its own, and imports it.
fn import(name: &str, bytes: &[u8]) -> Result<ExtractedText, ExtractError> {
    let folder: PathBuf = std::env::temp_dir().join(format!(
        "synapse-pdf-helper-{}-{}",
        std::process::id(),
        name.replace('.', "-")
    ));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(name);
    std::fs::write(&path, bytes).unwrap();
    let result = extract::read_file(&path, Path::new(HELPER));
    std::fs::remove_dir_all(&folder).unwrap();
    result
}

#[test]
fn the_helper_reads_each_page_and_reports_pages_without_text() {
    let read = import(
        "Lecture 3.pdf",
        &pdf(&[
            "Mitosis is cell division.\nIt produces two identical cells.",
            "",
            "Metaphase: chromosomes line up.",
        ]),
    )
    .unwrap();
    assert_eq!(read.kind, FileKind::Pdf);
    assert_eq!(read.title, "Lecture 3");
    assert_eq!(read.pages, Some(3));
    assert_eq!(read.pages_without_text, vec![2]);
    for text in [
        "Mitosis is cell division.",
        "It produces two identical cells.",
        "Metaphase: chromosomes line up.",
    ] {
        assert!(read.body.contains(text), "{text:?} in {:?}", read.body);
    }
    // In page order.
    assert!(read.body.find("Mitosis") < read.body.find("Metaphase"));
}

#[test]
fn a_pdf_with_no_text_at_all_is_refused() {
    assert!(matches!(
        import("scanned.pdf", &pdf(&["", ""])),
        Err(ExtractError::NoText)
    ));
}

#[test]
fn a_damaged_pdf_is_refused_and_the_app_carries_on() {
    // Whether the library reports it or crashes the helper, it's refused.
    for bytes in [
        &b"%PDF-1.4\nnot really a PDF"[..],
        &b"plain text with a .pdf name"[..],
    ] {
        let result = import("damaged.pdf", bytes);
        assert!(
            matches!(
                result,
                Err(ExtractError::Damaged(_) | ExtractError::HelperFailed(_))
            ),
            "{result:?}"
        );
    }
    // This process is still here, and the helper still works.
    assert!(import("fine.pdf", &pdf(&["Still fine."])).is_ok());
}

#[test]
fn a_helper_that_runs_too_long_is_stopped() {
    let result = extract::read_pdf_within(Path::new(HELPER), &pdf(&["Text"]), Duration::ZERO);
    assert!(
        matches!(result, Err(ExtractError::HelperFailed(_))),
        "{result:?}"
    );
}
