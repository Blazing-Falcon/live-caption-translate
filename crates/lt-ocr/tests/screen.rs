//! PP-OCRv6 on the rendered screen fixtures in `testdata/screen`. Real inference requires
//! LT_MODELS_DIR with the OCR model.

use lt_core::{
    engines::{TextLine, TextRecognizer},
    screen::{ImageView, Rect},
};
use lt_ocr::{OcrEngine, OCR_MODEL};
use serde::Deserialize;
use std::{path::PathBuf, time::Instant};

/// The reference pipeline's error rate on these fixtures, plus a margin for small differences in
/// boxes and resampling.
const MAX_CER: f64 = 0.0040 + 0.02;

#[derive(Deserialize)]
struct Truth {
    blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    id: u32,
    text: String,
    rect: Rect,
}

#[test]
fn every_fixture_block_is_found_and_read() {
    let Some(models) = std::env::var_os("LT_MODELS_DIR").map(PathBuf::from) else {
        eprintln!("Real model test skipped: set LT_MODELS_DIR to verified model directory");
        return;
    };
    lt_sherpa::initialize_shared_runtime(lt_sherpa::default_runtime_path().unwrap()).unwrap();
    let mut engine = OcrEngine::load(&models.join(OCR_MODEL), 4).unwrap();
    let screens = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/screen");
    let mut images: Vec<PathBuf> = std::fs::read_dir(&screens)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
        .collect();
    images.sort();
    assert!(!images.is_empty());
    let (mut edits, mut chars) = (0, 0);
    for path in &images {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let truth: Truth = serde_json::from_slice(
            &std::fs::read(screens.join(format!("{name}.truth.json"))).unwrap(),
        )
        .unwrap();
        let (width, height, bgra) = decode_png(path);
        let image = ImageView::new(width, height, width * 4, &bgra).unwrap();
        let started = Instant::now();
        let lines = engine.recognize(&image).unwrap();
        let ms = started.elapsed().as_millis();
        let (mut image_edits, mut image_chars) = (0, 0);
        for block in &truth.blocks {
            assert!(
                lines
                    .iter()
                    .any(|line| overlap(&line.rect, &block.rect) > 0),
                "{name}: no line overlaps block {}",
                block.id
            );
            let truth_text = fold(&block.text);
            let read = fold(&block_text(&lines, &truth.blocks, block.id));
            let block_edits = distance(&truth_text, &read);
            if block_edits > 0 {
                eprintln!(
                    "{name} block {}: {block_edits} edits, truth {} read {}",
                    block.id,
                    truth_text.iter().collect::<String>(),
                    read.iter().collect::<String>()
                );
            }
            image_edits += block_edits;
            image_chars += truth_text.len();
        }
        println!(
            "{name}: CER {:.4} ({image_edits}/{image_chars}), {} lines, {ms} ms",
            image_edits as f64 / image_chars as f64,
            lines.len()
        );
        edits += image_edits;
        chars += image_chars;
    }
    let cer = edits as f64 / chars as f64;
    println!("all fixtures: CER {cer:.4} ({edits}/{chars})");
    assert!(cer <= MAX_CER, "CER {cer:.4} is above {MAX_CER}");
}

fn decode_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let bytes = std::fs::read(path).unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    let channels = info.color_type.samples();
    assert!(channels >= 3, "{}: grayscale fixture", path.display());
    let bgra = buffer[..info.buffer_size()]
        .chunks_exact(channels)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 255])
        .collect();
    (info.width, info.height, bgra)
}

fn overlap(a: &Rect, b: &Rect) -> i64 {
    let w = i64::from(a.right().min(b.right())) - i64::from(a.x.max(b.x));
    let h = i64::from(a.bottom().min(b.bottom())) - i64::from(a.y.max(b.y));
    w.max(0) * h.max(0)
}

/// The lines whose area lies at least half in `id` and more in it than in any other block, in
/// reading order.
fn block_text(lines: &[TextLine], blocks: &[Block], id: u32) -> String {
    let mut mine: Vec<&TextLine> = lines
        .iter()
        .filter(|line| {
            let best = blocks
                .iter()
                .max_by_key(|block| overlap(&line.rect, &block.rect))
                .unwrap();
            let area = i64::from(line.rect.w) * i64::from(line.rect.h);
            best.id == id && 2 * overlap(&line.rect, &best.rect) >= area
        })
        .collect();
    mine.sort_by_key(|line| ((line.rect.y as f32 / 8.0).round() as i32, line.rect.x));
    mine.iter().map(|line| line.text.as_str()).collect()
}

/// Whitespace removed; full-width ASCII, curly quotes and dashes folded to ASCII.
fn fold(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap(),
            '“' | '”' => '"',
            '‘' | '’' => '\'',
            '–' | '—' => '-',
            _ => c,
        })
        .collect()
}

fn distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(x != y));
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}
