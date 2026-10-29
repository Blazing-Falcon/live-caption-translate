//! Grouping of recorded OCR lines against the rendered fixtures' truth: every truth block
//! becomes exactly one block, and no block holds lines of two truth blocks.

use lt_core::{
    engines::TextLine,
    screen::{blocks, ImageView, Rect},
};
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

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
    text: String,
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
    for name in &names {
        let lines: Recorded = serde_json::from_slice(
            &std::fs::read(recorded.join(format!("{name}.lines.json"))).unwrap(),
        )
        .unwrap();
        let truth: Truth = serde_json::from_slice(
            &std::fs::read(truths.join(format!("{name}.truth.json"))).unwrap(),
        )
        .unwrap();
        let white = vec![255; lines.width as usize * lines.height as usize * 4];
        let image = ImageView::new(lines.width, lines.height, lines.width * 4, &white).unwrap();
        let found = blocks(&image, &lines.lines);
        println!(
            "{name}: {} blocks, truth {}",
            found.len(),
            truth.blocks.len()
        );

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
            let truth_breaks = truth.blocks[id as usize].text.matches('\n').count();
            if block.source.matches('\n').count() != truth_breaks {
                problems.push(format!(
                    "{name}: block {} {:?} has a different number of line breaks than truth block {id} (truth has {truth_breaks})",
                    block.id, block.source
                ));
            }
        }
        for truth_block in &truth.blocks {
            let ours = produced.get(&truth_block.id).map_or(0, Vec::len);
            if ours != 1 {
                problems.push(format!(
                    "{name}: truth block {} {:?} became {ours} blocks",
                    truth_block.id,
                    truth_block.text.chars().take(12).collect::<String>()
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
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
