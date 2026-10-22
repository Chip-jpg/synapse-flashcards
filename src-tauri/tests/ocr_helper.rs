//! Text recognition through the helper process (see `ocr.rs`): Synapse,
//! started by itself to read pictures and scanned pages. These run the real
//! Synapse binary as the helper, which the unit tests can't.

use std::path::Path;
use std::time::Duration;

use lopdf::{dictionary, Document, Object, Stream};
use synapse_scaffold_lib::ocr::{self, Job, PageOutcome, PageProblem, RunError};
use tokio_util::sync::CancellationToken;

const HELPER: &str = env!("CARGO_BIN_EXE_synapse-scaffold");

fn sample_png() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/ocr/page.png"
    ))
    .unwrap()
}

/// A PDF of `count` pages, each drawing the sample page as a JPEG.
fn scanned_pdf(count: usize) -> Vec<u8> {
    let picture = image::load_from_memory(&sample_png()).unwrap().into_luma8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&picture)
        .unwrap();
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut image = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => picture.width() as i64,
            "Height" => picture.height() as i64,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpeg,
    );
    image.allows_compression = false;
    let image_id = doc.add_object(image);
    let mut kids: Vec<Object> = Vec::new();
    for _ in 0..count {
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            b"q 500 0 0 165 50 600 cm /Im1 Do Q".to_vec(),
        ));
        kids.push(
            doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image_id } },
            })
            .into(),
        );
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count as i64,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn run(
    job: &Job,
    bytes: &[u8],
    stop: &CancellationToken,
    timeout: Duration,
) -> (Vec<(u32, PageOutcome)>, Result<(), RunError>) {
    let mut answers = Vec::new();
    let result = ocr::run_helper(
        Path::new(HELPER),
        job,
        bytes,
        stop,
        timeout,
        |page, outcome| answers.push((page, outcome)),
    );
    (answers, result)
}

const EXPECTED: &str = "Photosynthesis converts light energy into chemical energy.\n\
                        The light reactions take place in the thylakoid membranes.\n\n\
                        The Calvin cycle fixes carbon dioxide into sugars.";

#[test]
fn the_helper_reads_a_picture() {
    let (answers, result) = run(
        &Job::Picture,
        &sample_png(),
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    assert_eq!(answers.len(), 1);
    let (page, PageOutcome::Read(reading)) = &answers[0] else {
        panic!("{answers:?}");
    };
    assert_eq!(*page, 0);
    assert_eq!(reading.text(), EXPECTED);

    let (answers, result) = run(
        &Job::Picture,
        b"not a picture",
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    assert_eq!(
        answers,
        vec![(0, PageOutcome::Problem(PageProblem::UnreadablePicture))]
    );
}

#[test]
fn the_helper_reads_pages_of_a_pdf_in_the_order_asked() {
    let (answers, result) = run(
        &Job::Pdf {
            pages: vec![2, 1, 5],
        },
        &scanned_pdf(2),
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    let pages: Vec<u32> = answers.iter().map(|(page, _)| *page).collect();
    assert_eq!(pages, vec![2, 1, 5]);
    for (_, outcome) in &answers[..2] {
        let PageOutcome::Read(reading) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(reading.text(), EXPECTED);
    }
    assert_eq!(answers[2].1, PageOutcome::Problem(PageProblem::NoSuchPage));
}

#[test]
fn a_damaged_pdf_is_refused_and_the_app_carries_on() {
    let (answers, result) = run(
        &Job::Pdf { pages: vec![1] },
        b"%PDF-1.7 this is not a PDF",
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    assert!(answers.is_empty());
    assert!(matches!(result, Err(RunError::Unreadable(_))));
}

#[test]
fn stopping_ends_the_helper_and_keeps_the_pages_already_answered() {
    let stop = CancellationToken::new();
    let mut answered = 0;
    let result = ocr::run_helper(
        Path::new(HELPER),
        &Job::Pdf {
            pages: (1..=30).collect(),
        },
        &scanned_pdf(30),
        &stop,
        ocr::PAGE_TIMEOUT,
        |_, _| {
            answered += 1;
            stop.cancel();
        },
    );
    assert!(matches!(result, Err(RunError::Stopped)));
    assert!((1..30).contains(&answered), "{answered}");
}

#[test]
fn a_helper_that_spends_too_long_on_a_page_is_stopped() {
    let (_, result) = run(
        &Job::Pdf { pages: vec![1] },
        &scanned_pdf(1),
        &CancellationToken::new(),
        Duration::from_millis(1),
    );
    assert!(matches!(result, Err(RunError::HelperFailed(_))));
}

#[test]
fn the_helper_hands_back_a_picture_with_the_asked_part_framed() {
    let marks = vec![ocr::Mark {
        left: 0.0,
        top: 0.0,
        width: 0.5,
        height: 0.5,
    }];
    let (answers, result) = run(
        &Job::MarkedPictureForModel { marks },
        &sample_png(),
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    let [(0, PageOutcome::Picture(png))] = answers.as_slice() else {
        panic!("{answers:?}");
    };
    let original = image::load_from_memory(&sample_png()).unwrap().into_rgb8();
    let sent = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .unwrap()
        .into_rgb8();
    assert_eq!(sent.dimensions(), original.dimensions());
    // The frame is drawn just inside the mark; the rest is the picture.
    assert_eq!(sent.get_pixel(0, 0).0, [255, 255, 255]);
    assert_eq!(sent.get_pixel(2, 40).0, [230, 0, 30]);
    let (width, height) = sent.dimensions();
    assert_eq!(
        sent.get_pixel(width - 1, height - 1),
        original.get_pixel(width - 1, height - 1)
    );

    // Something that isn't a picture is a problem, as for the other jobs.
    let (answers, result) = run(
        &Job::MarkedPictureForModel { marks: Vec::new() },
        b"not a picture",
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    assert!(
        matches!(answers.as_slice(), [(0, PageOutcome::Problem(_))]),
        "{answers:?}"
    );
}

#[test]
fn the_helper_hands_back_pictures_for_a_vision_model_without_reading_them() {
    let (answers, result) = run(
        &Job::PictureForModel,
        &sample_png(),
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    let [(0, PageOutcome::Picture(png))] = answers.as_slice() else {
        panic!("{answers:?}");
    };
    let original = image::load_from_memory(&sample_png()).unwrap();
    let sent = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap();
    assert_eq!(
        (sent.width(), sent.height()),
        (original.width(), original.height())
    );

    let (answers, result) = run(
        &Job::PageForModel { page: 2 },
        &scanned_pdf(2),
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    let [(2, PageOutcome::Picture(png))] = answers.as_slice() else {
        panic!("{answers:?}");
    };
    let page = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap();
    assert!(page.width().max(page.height()) <= ocr::MODEL_MAX_SIDE);
    assert_eq!(page.width(), original.width());

    let (answers, result) = run(
        &Job::PageForModel { page: 3 },
        &scanned_pdf(2),
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    result.unwrap();
    assert_eq!(
        answers,
        vec![(3, PageOutcome::Problem(PageProblem::NoSuchPage))]
    );

    let (answers, result) = run(
        &Job::PageForModel { page: 1 },
        b"%PDF-1.7 this is not a PDF",
        &CancellationToken::new(),
        ocr::PAGE_TIMEOUT,
    );
    assert!(answers.is_empty());
    assert!(matches!(result, Err(RunError::Unreadable(_))));
}
