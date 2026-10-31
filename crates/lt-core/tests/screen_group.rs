//! Recorded grouping contracts and the rendered fixtures' truth.

use lt_core::{
    engines::TextLine,
    screen::{
        filters,
        group::{blocks_from_groups_direction, group_screenshot},
        ink,
        line::RawLine,
        Direction, ImageView, Rect,
    },
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::BufReader,
    path::PathBuf,
};

/// Truth blocks the grouping splits into more than one block on current recognizer output:
/// (page@scale, truth block id, blocks produced). The test fails when one stops or a new one appears.
const KNOWN_MISSES: &[(&str, u32, usize)] = &[
    ("chat@1", 1, 2),
    ("chat@1.5", 1, 2),
    ("subtitle@1", 0, 2),
    ("subtitle@1.5", 0, 2),
];

#[derive(Deserialize)]
struct Recorded {
    width: u32,
    height: u32,
    lines: Vec<TextLine>,
}

#[derive(Deserialize)]
struct Truth {
    blocks: Vec<TruthBlock>,
}

#[derive(Deserialize)]
struct TruthBlock {
    id: u32,
    rect: Rect,
}

#[test]
fn recorded_lines_group_into_the_truth_blocks() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let recorded = root.join("tests/fixtures/screen");
    let truths = root.join("../../testdata/screen");
    let mut names: Vec<String> = std::fs::read_dir(&recorded)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".lines.json").map(str::to_owned)
        })
        .collect();
    names.sort();
    assert!(!names.is_empty());
    let mut problems = Vec::new();
    let mut misses = BTreeSet::new();
    for name in &names {
        let lines: Recorded = serde_json::from_slice(
            &std::fs::read(recorded.join(format!("{name}.lines.json"))).unwrap(),
        )
        .unwrap();
        let truth: Truth = serde_json::from_slice(
            &std::fs::read(truths.join(format!("{name}.truth.json"))).unwrap(),
        )
        .unwrap();
        let (width, height, bgra) = decode(&truths.join(format!("{name}.png")));
        assert_eq!((width, height), (lines.width, lines.height), "{name}");
        let image = ImageView::new(width, height, width * 4, &bgra).unwrap();
        let raw: Vec<RawLine> = lines.lines.iter().map(RawLine::from).collect();
        let mut facts = filters::prepare(&raw);
        ink::annotate(&image, &mut facts);
        let groups = group_screenshot(&facts);
        let found = blocks_from_groups_direction(&image, &groups, Direction::Horizontal);

        let mut produced: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for block in &found {
            let owners: Vec<Option<u32>> = block
                .lines
                .iter()
                .map(|line| owner(line, &truth.blocks))
                .collect();
            let first = owners[0];
            if first.is_none() || owners.iter().any(|owner| *owner != first) {
                problems.push(format!(
                    "{name}: block {} {:?} holds lines of truth blocks {owners:?}",
                    block.id, block.source
                ));
                continue;
            }
            let id = first.unwrap();
            produced.entry(id).or_default().push(block.id);
        }
        for truth_block in &truth.blocks {
            let ours = produced.get(&truth_block.id).map_or(0, Vec::len);
            if ours != 1 {
                misses.insert((name.clone(), truth_block.id, ours));
            }
        }
    }
    let expected: BTreeSet<_> = KNOWN_MISSES
        .iter()
        .map(|(name, id, ours)| ((*name).to_owned(), *id, *ours))
        .collect();
    for miss in misses.difference(&expected) {
        problems.push(format!("new miss {miss:?}"));
    }
    for miss in expected.difference(&misses) {
        problems.push(format!("known miss no longer happens {miss:?}"));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn decode(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path).unwrap()));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    let pixels = &pixels[..info.buffer_size()];
    let mut bgra = Vec::with_capacity((info.width * info.height * 4) as usize);
    match info.color_type {
        png::ColorType::Rgba => {
            for c in pixels.as_chunks::<4>().0 {
                bgra.extend_from_slice(&[c[2], c[1], c[0], c[3]]);
            }
        }
        png::ColorType::Rgb => {
            for c in pixels.as_chunks::<3>().0 {
                bgra.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        other => panic!("unexpected pixels: {other:?}"),
    }
    (info.width, info.height, bgra)
}

/// The truth block that holds at least half of the line.
fn owner(line: &Rect, truth: &[TruthBlock]) -> Option<u32> {
    let area = u64::from(line.w) * u64::from(line.h);
    truth
        .iter()
        .map(|block| (overlap(line, &block.rect), block.id))
        .max()
        .filter(|(shared, _)| 2 * shared >= area)
        .map(|(_, id)| id)
}

fn overlap(a: &Rect, b: &Rect) -> u64 {
    let w = i64::from(a.right().min(b.right())) - i64::from(a.x.max(b.x));
    let h = i64::from(a.bottom().min(b.bottom())) - i64::from(a.y.max(b.y));
    (w.max(0) * h.max(0)) as u64
}
