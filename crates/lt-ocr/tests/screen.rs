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

const BASELINE_PAGES: [&str; 6] = ["article", "chat", "dense", "mixed", "subtitle", "ui-panel"];

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
fn horizontal_baseline_blocks_are_found_and_read() {
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
        .filter(|path| {
            let name = path.file_stem().unwrap().to_string_lossy();
            BASELINE_PAGES
                .iter()
                .any(|page| name == format!("{page}@1") || name == format!("{page}@1.5"))
        })
        .collect();
    images.sort();
    assert_eq!(images.len(), BASELINE_PAGES.len() * 2);
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
    tilted_groups(&mut engine, &screens);
    vertical_groups(&mut engine, &screens);
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

#[derive(Deserialize)]
struct TiltedCase {
    name: String,
    groups: Vec<(Vec<usize>, String, bool)>,
}

fn tilted_groups(engine: &mut OcrEngine, screens: &std::path::Path) {
    use lt_core::screen::{filters, group, ink, line::RawLine};
    use std::collections::HashMap;
    let cases: Vec<TiltedCase> =
        serde_json::from_str(include_str!("fixtures/tilted-groups.json")).unwrap();
    for case in cases {
        let (w, h, pixels) = decode_png(&screens.join(format!("{}.png", case.name)));
        let image = ImageView::new(w, h, w * 4, &pixels).unwrap();
        let lines = engine.recognize(&image).unwrap();
        let raw: Vec<_> = lines.iter().map(RawLine::from).collect();
        let mut facts = filters::prepare(&raw);
        ink::annotate(&image, &mut facts);
        let report = group::report_screenshot(&facts, &group::profile::GENERAL, true);
        let sources: HashMap<_, _> = report
            .trace
            .lines
            .iter()
            .map(|line| (line.id.as_str(), &line.source_ids))
            .collect();
        let actual: Vec<_> = report
            .groups
            .iter()
            .zip(&report.trace.groups)
            .map(|(group, ids)| {
                let members = ids
                    .iter()
                    .flat_map(|id| {
                        sources[id.as_str()]
                            .iter()
                            .map(|id| id[1..].parse::<usize>().unwrap())
                    })
                    .collect();
                (members, group.text.clone(), group.tilt.is_some())
            })
            .collect();
        assert_eq!(actual, case.groups, "{}: recognition/grouping", case.name);
        println!(
            "{}: exact membership/text/tilt for {} groups",
            case.name,
            actual.len()
        );
    }
}

#[derive(Deserialize)]
struct VerticalCase {
    name: String,
    raw: Vec<TextLine>,
    groups: Vec<(Vec<usize>, String, bool)>,
}

fn vertical_groups(engine: &mut OcrEngine, screens: &std::path::Path) {
    use lt_core::screen::{vertical, Direction};
    let cases: Vec<VerticalCase> =
        serde_json::from_str(include_str!("fixtures/vertical-groups.json")).unwrap();
    let (mut matched, mut texts, mut total) = (0, 0, 0);
    for case in cases {
        let (w, h, pixels) = decode_png(&screens.join(format!("{}.png", case.name)));
        let image = ImageView::new(w, h, w * 4, &pixels).unwrap();
        let lines = engine
            .recognize_direction(&image, Direction::Vertical)
            .unwrap();
        assert_eq!(lines.len(), case.raw.len(), "{}: box count", case.name);
        let mut taken = vec![false; lines.len()];
        for expected in &case.raw {
            let found = lines.iter().enumerate().find(|(index, line)| {
                if taken[*index] {
                    return false;
                }
                let shared = overlap(&line.rect, &expected.rect);
                let union = i64::from(line.rect.w) * i64::from(line.rect.h)
                    + i64::from(expected.rect.w) * i64::from(expected.rect.h)
                    - shared;
                shared as f64 / union as f64 >= 0.9
            });
            if let Some((index, line)) = found {
                taken[index] = true;
                matched += 1;
                texts += usize::from(line.text == expected.text);
            }
            total += 1;
        }
        let facts = vertical::prepare_lines(&lines);
        let groups = vertical::grouping::group(facts.clone(), Some(&image), Some(true));
        let actual: Vec<_> = groups
            .iter()
            .map(|group| {
                let mut members: Vec<_> = facts
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| {
                        group.layout_bounds.contains_rect(line.layout_bounds)
                            && group.text.contains(&line.text)
                    })
                    .map(|(i, _)| i)
                    .collect();
                members.sort_by(|a, b| {
                    let (a, b) = (facts[*a].layout_bounds, facts[*b].layout_bounds);
                    if group.runs_across {
                        a.y.total_cmp(&b.y).then_with(|| a.x.total_cmp(&b.x))
                    } else {
                        b.x.total_cmp(&a.x).then_with(|| a.y.total_cmp(&b.y))
                    }
                });
                (members, group.text.clone(), group.runs_across)
            })
            .collect();
        assert_eq!(
            actual, case.groups,
            "{}: column membership/text/direction",
            case.name
        );
        println!(
            "{}: exact membership/text/direction for {} groups",
            case.name,
            actual.len()
        );
    }
    assert!(
        matched as f64 / total as f64 >= 0.98,
        "vertical boxes {matched}/{total}"
    );
    assert!(
        texts as f64 / matched as f64 >= 0.95,
        "vertical texts {texts}/{matched}"
    );
}
