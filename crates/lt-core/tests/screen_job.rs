//! The screen job's event order, cancelling and cache, with a fake reader and translator.

use lt_core::{
    engines::{TextLine, TextRecognizer, TranslatorCaps},
    error::{Error, Result},
    events::FailReason,
    screen::{
        run_job, run_job_with_output, ImageView, JobOutput, Rect, ScreenEvent, TranslationCache,
    },
};
use std::sync::atomic::{AtomicBool, Ordering};

struct FakeReader(Option<Vec<TextLine>>);

impl TextRecognizer for FakeReader {
    fn recognize(&mut self, _: &ImageView<'_>) -> Result<Vec<TextLine>> {
        self.0
            .clone()
            .ok_or_else(|| Error::Engine("The text recognizer is missing".into()))
    }
}

/// A line of text 300 pixels wide whose top is `y`; a gap this large keeps every line in a block
/// of its own.
fn line(y: i32, text: &str) -> TextLine {
    TextLine {
        rect: Rect {
            x: 10,
            y,
            w: (lt_core::screen::line::glyphs_in(text) as u32 * 20).clamp(20, 280),
            h: 20,
        },
        text: text.into(),
        score: 0.99,
        ..TextLine::default()
    }
}

#[test]
fn vertical_job_keeps_source_pixels_and_translates_columns_right_to_left() {
    use lt_core::screen::Direction;
    struct Columns;
    impl TextRecognizer for Columns {
        fn recognize(&mut self, _: &ImageView<'_>) -> lt_core::error::Result<Vec<TextLine>> {
            panic!("direction was lost")
        }
        fn recognize_direction(
            &mut self,
            image: &ImageView<'_>,
            direction: Direction,
        ) -> lt_core::error::Result<Vec<TextLine>> {
            assert_eq!(direction, Direction::Vertical);
            assert_eq!((image.width, image.height), (120, 180));
            assert_eq!(&image.row(11)[28..32], &[0, 0, 255, 255]);
            Ok([(36, "世界"), (60, "你好")]
                .map(|(x, text)| TextLine {
                    rect: Rect {
                        x,
                        y: 20,
                        w: 20,
                        h: 60,
                    },
                    text: text.into(),
                    score: 0.99,
                    ..TextLine::default()
                })
                .to_vec())
        }
    }
    let mut pixels = vec![255; 120 * 180 * 4];
    pixels[(11 * 120 + 7) * 4..][..4].copy_from_slice(&[0, 0, 255, 255]);
    let image = ImageView::new(120, 180, 480, &pixels).unwrap();
    let mut events = Vec::new();
    run_job_with_output(
        9,
        &image,
        &mut Columns,
        &mut |text| {
            assert_eq!(text, ["你好世界"]);
            Ok(vec!["Hello world".into()])
        },
        &AtomicBool::new(false),
        &mut TranslationCache::default(),
        JobOutput {
            translator_caps: TranslatorCaps::default(),
            direction: Direction::Vertical,
            debug: false,
            prepare: None,
            emit: &mut |event| events.push(event),
        },
    );
    assert_eq!(
        described(&events),
        ["reading", "blocks 1", "translated 0", "done"]
    );
    let ScreenEvent::Blocks { blocks, .. } = &events[1] else {
        panic!("missing blocks")
    };
    assert_eq!(blocks[0].source, "你好世界");
    assert_eq!(
        blocks[0].rect,
        Rect {
            x: 36,
            y: 20,
            w: 44,
            h: 60
        }
    );
}

fn translation_failed() -> Error {
    Error::Translation {
        reason: FailReason::Timeout,
        message: "Translation took too long".into(),
    }
}

/// Runs a job over `lines`; `translate` also gets the cancel flag. Returns the events and the
/// pieces the translator was asked for.
fn run(
    lines: Option<Vec<TextLine>>,
    cache: &mut TranslationCache,
    cancel: &AtomicBool,
    mut translate: impl FnMut(&str, &AtomicBool) -> Result<String>,
    mut on_event: impl FnMut(&ScreenEvent, &AtomicBool),
) -> (Vec<ScreenEvent>, Vec<String>) {
    let pixels = vec![255; 300 * 400 * 4];
    let image = ImageView::new(300, 400, 300 * 4, &pixels).unwrap();
    let mut requests = Vec::new();
    let mut events = Vec::new();
    run_job(
        7,
        &image,
        &mut FakeReader(lines),
        &mut |piece| {
            assert_eq!(piece.len(), 1);
            requests.push(piece[0].to_owned());
            translate(piece[0], cancel).map(|text| vec![text])
        },
        cancel,
        cache,
        &mut |event| {
            on_event(&event, cancel);
            events.push(event);
        },
    );
    (events, requests)
}

fn describe(event: &ScreenEvent) -> String {
    match event {
        ScreenEvent::Reading { .. } => "reading".into(),
        ScreenEvent::Blocks { blocks, .. } => format!("blocks {}", blocks.len()),
        ScreenEvent::Translated { id, .. } => format!("translated {id}"),
        ScreenEvent::Failed { id, .. } => format!("failed {id}"),
        ScreenEvent::Done { .. } => "done".into(),
        ScreenEvent::Error { .. } => "error".into(),
    }
}

fn described(events: &[ScreenEvent]) -> Vec<String> {
    events.iter().map(describe).collect()
}

#[test]
fn events_follow_the_order_contract() {
    let lines = vec![
        line(300, "谢谢"),
        line(10, "你好"),
        line(110, "Hello"),
        line(210, "再见"),
    ];
    let (events, requests) = run(
        Some(lines),
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |piece, _| match piece {
            "再见" => Err(translation_failed()),
            _ => Ok(format!("en:{piece}")),
        },
        |_, _| {},
    );
    assert_eq!(
        described(&events),
        [
            "reading",
            "blocks 4",
            "translated 0",
            "failed 2",
            "translated 3",
            "done"
        ]
    );
    let ScreenEvent::Blocks { blocks, .. } = &events[1] else {
        unreachable!()
    };
    let flags: Vec<(u32, bool)> = blocks
        .iter()
        .map(|block| (block.id, block.translate))
        .collect();
    assert_eq!(flags, [(0, true), (1, false), (2, true), (3, true)]);
    // A block without a Han character sends no request.
    assert_eq!(requests, ["你好", "再见", "谢谢"]);
    assert!(matches!(&events[3],
        ScreenEvent::Failed { message, .. } if message == "Translation took too long"));
    assert!(events.iter().all(|event| match event {
        ScreenEvent::Reading { job }
        | ScreenEvent::Blocks { job, .. }
        | ScreenEvent::Translated { job, .. }
        | ScreenEvent::Failed { job, .. }
        | ScreenEvent::Done { job, .. }
        | ScreenEvent::Error { job, .. } => *job == 7,
    }));
}

#[test]
fn a_job_cancelled_before_it_starts_emits_nothing() {
    let (events, requests) = run(
        None,
        &mut TranslationCache::default(),
        &AtomicBool::new(true),
        |_, _| panic!("cancelled job must not translate"),
        |_, _| {},
    );
    assert!(events.is_empty());
    assert!(requests.is_empty());
}

#[test]
fn cancelling_during_paint_preparation_emits_no_blocks_or_translation() {
    let pixels = vec![255; 300 * 400 * 4];
    let image = ImageView::new(300, 400, 1200, &pixels).unwrap();
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    let mut prepared = false;
    run_job_with_output(
        7,
        &image,
        &mut FakeReader(Some(vec![line(10, "你好")])),
        &mut |_| panic!("cancelled paint must not translate"),
        &cancel,
        &mut TranslationCache::default(),
        JobOutput {
            translator_caps: TranslatorCaps::default(),
            direction: lt_core::screen::Direction::Horizontal,
            debug: false,
            emit: &mut |event| events.push(event),
            prepare: Some(&mut |_, groups, blocks| {
                assert_eq!(groups.len(), 1);
                assert_eq!(blocks.len(), 1);
                prepared = true;
                cancel.store(true, Ordering::Release);
                Ok(())
            }),
        },
    );
    assert!(prepared);
    assert_eq!(described(&events), ["reading"]);
}

#[test]
fn debug_lines_include_detections_rejected_before_grouping_only_when_requested() {
    let pixels = vec![255; 300 * 400 * 4];
    let image = ImageView::new(300, 400, 1200, &pixels).unwrap();
    let mut rejected = line(100, "好");
    rejected.rect.w = 280;
    let input = vec![line(10, "你好"), rejected];
    for debug in [false, true] {
        let mut events = Vec::new();
        run_job_with_output(
            7,
            &image,
            &mut FakeReader(Some(input.clone())),
            &mut |_| Ok(vec!["Hello".into()]),
            &AtomicBool::new(false),
            &mut TranslationCache::default(),
            JobOutput {
                translator_caps: TranslatorCaps::default(),
                direction: lt_core::screen::Direction::Horizontal,
                debug,
                emit: &mut |event| events.push(event),
                prepare: None,
            },
        );
        let ScreenEvent::Blocks {
            blocks, ocr_lines, ..
        } = &events[1]
        else {
            panic!("missing blocks")
        };
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            *ocr_lines,
            debug.then(|| input.iter().map(|line| line.rect).collect())
        );
        assert_eq!(
            described(&events),
            ["reading", "blocks 1", "translated 0", "done"]
        );
    }
}

#[test]
fn an_unreadable_image_ends_with_one_error_after_reading() {
    let (events, requests) = run(
        None,
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |_, _| unreachable!(),
        |_, _| {},
    );
    assert_eq!(described(&events), ["reading", "error"]);
    assert!(matches!(&events[1],
        ScreenEvent::Error { message, .. } if message == "The text recognizer is missing"));
    assert!(requests.is_empty());
}

#[test]
fn cancelling_stops_the_requests_and_the_events() {
    let lines = vec![line(10, "你好"), line(110, "再见"), line(210, "谢谢")];

    // Cancelled when the first translation is reported: no second request, no `Done`.
    let (events, requests) = run(
        Some(lines.clone()),
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |piece, _| Ok(format!("en:{piece}")),
        |event, cancel| {
            if matches!(event, ScreenEvent::Translated { .. }) {
                cancel.store(true, Ordering::Release);
            }
        },
    );
    assert_eq!(described(&events), ["reading", "blocks 3", "translated 0"]);
    assert_eq!(requests, ["你好"]);

    // Cancelled during a request: its answer is dropped.
    let (events, requests) = run(
        Some(lines),
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |piece, cancel| {
            cancel.store(true, Ordering::Release);
            Ok(format!("en:{piece}"))
        },
        |_, _| {},
    );
    assert_eq!(described(&events), ["reading", "blocks 3"]);
    assert_eq!(requests, ["你好"]);

    // Cancelled when the last block is reported: no `Done`.
    let (events, _) = run(
        Some(vec![line(10, "你好")]),
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |piece, _| Ok(format!("en:{piece}")),
        |event, cancel| {
            if matches!(event, ScreenEvent::Translated { .. }) {
                cancel.store(true, Ordering::Release);
            }
        },
    );
    assert_eq!(described(&events), ["reading", "blocks 1", "translated 0"]);
}

#[test]
fn repeated_text_is_translated_once() {
    let lines = vec![
        line(10, "确定"),
        line(110, "取消"),
        line(210, "确定"),
        line(310, "确定"),
    ];
    let mut cache = TranslationCache::default();
    let (events, requests) = run(
        Some(lines.clone()),
        &mut cache,
        &AtomicBool::new(false),
        |piece, _| Ok(format!("en:{piece}")),
        |_, _| {},
    );
    assert_eq!(requests, ["确定", "取消"]);
    let texts: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ScreenEvent::Translated { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["en:确定", "en:取消", "en:确定", "en:确定"]);

    // The cache outlives the job.
    let (events, requests) = run(
        Some(lines),
        &mut cache,
        &AtomicBool::new(false),
        |_, _| unreachable!(),
        |_, _| {},
    );
    assert!(requests.is_empty());
    assert_eq!(events.len(), 7);
}

#[test]
fn a_long_block_is_sent_in_pieces_and_a_failed_piece_fails_only_its_block() {
    let sentence = format!("{}。", "一".repeat(50));
    let long = sentence.repeat(3);
    let (events, requests) = run(
        Some(vec![line(10, &long), line(110, "谢谢")]),
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |piece, _| match piece.chars().count() {
            102 => Ok("first half".into()),
            _ if piece == sentence => Err(translation_failed()),
            _ => Ok("thanks".into()),
        },
        |_, _| {},
    );
    assert_eq!(
        requests,
        [format!("{sentence}{sentence}"), sentence, "谢谢".into()]
    );
    assert_eq!(
        described(&events),
        ["reading", "blocks 2", "failed 0", "translated 1", "done"]
    );
}

#[test]
fn the_pieces_of_a_block_are_joined_with_a_space() {
    let text = format!("{}。{}。", "二".repeat(60), "三".repeat(60));
    let (events, requests) = run(
        Some(vec![line(10, &text)]),
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |_, _| Ok(" part ".into()),
        |_, _| {},
    );
    assert_eq!(requests.len(), 2);
    assert!(matches!(&events[2],
        ScreenEvent::Translated { text, .. } if text == "part part"));
}

fn run_batch(
    lines: Vec<TextLine>,
    caps: TranslatorCaps,
    cache: &mut TranslationCache,
    mut translate: impl FnMut(&[&str]) -> Result<Vec<String>>,
) -> Vec<ScreenEvent> {
    let height = lines
        .iter()
        .map(|line| line.rect.y as u32 + line.rect.h)
        .max()
        .unwrap_or(1);
    let pixels = vec![255; 300 * height as usize * 4];
    let image = ImageView::new(300, height, 1200, &pixels).unwrap();
    let mut events = Vec::new();
    run_job_with_output(
        8,
        &image,
        &mut FakeReader(Some(lines)),
        &mut translate,
        &AtomicBool::new(false),
        cache,
        JobOutput {
            translator_caps: caps,
            direction: lt_core::screen::Direction::Horizontal,
            debug: false,
            prepare: None,
            emit: &mut |event| events.push(event),
        },
    );
    events
}

fn many_lines() -> Vec<TextLine> {
    (0..20)
        .map(|index| line(10 + index * 100, &format!("测试第{index}条")))
        .collect()
}

#[test]
fn batches_respect_limits_and_complete_blocks_in_reading_order() {
    let mut calls = Vec::new();
    let events = run_batch(
        many_lines(),
        TranslatorCaps {
            max_batch_items: 8,
            max_batch_chars: 1000,
            ..Default::default()
        },
        &mut TranslationCache::default(),
        |texts| {
            calls.push(
                texts
                    .iter()
                    .map(|text| text.to_string())
                    .collect::<Vec<_>>(),
            );
            Ok(texts.iter().map(|text| format!("en:{text}")).collect())
        },
    );
    assert_eq!(calls.iter().map(Vec::len).collect::<Vec<_>>(), [8, 8, 4]);
    assert_eq!(
        calls.into_iter().flatten().collect::<Vec<_>>(),
        many_lines()
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
    );
    let mut expected = vec!["reading".to_owned(), "blocks 20".to_owned()];
    expected.extend((0..20).map(|id| format!("translated {id}")));
    expected.push("done".into());
    assert_eq!(described(&events), expected);
}

#[test]
fn a_short_batch_answer_fails_only_its_blocks_and_later_batches_continue() {
    let mut calls = 0;
    let events = run_batch(
        many_lines(),
        TranslatorCaps {
            max_batch_items: 8,
            max_batch_chars: 1000,
            ..Default::default()
        },
        &mut TranslationCache::default(),
        |texts| {
            calls += 1;
            let count = if calls == 2 {
                texts.len() - 1
            } else {
                texts.len()
            };
            Ok(vec!["translated".into(); count])
        },
    );
    assert_eq!(calls, 3);
    let failed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ScreenEvent::Failed { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(failed, (8..16).collect::<Vec<_>>());
    assert!(matches!(events[21], ScreenEvent::Translated { id: 19, .. }));
    assert!(matches!(events.last(), Some(ScreenEvent::Done { .. })));
}

#[test]
fn batches_deduplicate_pieces_and_reuse_cached_answers_across_jobs() {
    let long = format!("{}。{}。", "二".repeat(60), "三".repeat(60));
    let lines = vec![line(10, &long), line(110, "确定"), line(210, "确定")];
    let caps = TranslatorCaps {
        max_batch_items: 8,
        max_batch_chars: 1000,
        ..Default::default()
    };
    let mut cache = TranslationCache::default();
    let mut requests = Vec::new();
    let events = run_batch(lines.clone(), caps.clone(), &mut cache, |texts| {
        requests.push(
            texts
                .iter()
                .map(|text| text.to_string())
                .collect::<Vec<_>>(),
        );
        Ok(texts.iter().map(|_| " part ".into()).collect())
    });
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].len(), 3);
    assert_eq!(
        requests[0],
        [
            format!("{}。", "二".repeat(60)),
            format!("{}。", "三".repeat(60)),
            "确定".into()
        ]
    );
    assert!(matches!(&events[2], ScreenEvent::Translated { text, .. } if text == "part part"));
    let cached = run_batch(lines, caps, &mut cache, |_| {
        panic!("cache must avoid the network")
    });
    assert_eq!(described(&cached), described(&events));
}

#[test]
fn character_budget_counts_utf16_and_cuts_before_the_next_piece() {
    let mut sizes = Vec::new();
    let events = run_batch(
        vec![line(10, "你好😀"), line(110, "世界😀"), line(210, "谢谢😀")],
        TranslatorCaps {
            max_batch_items: 8,
            max_batch_chars: 8,
            ..Default::default()
        },
        &mut TranslationCache::default(),
        |texts| {
            sizes.push(
                texts
                    .iter()
                    .map(|text| text.encode_utf16().count())
                    .sum::<usize>(),
            );
            Ok(vec!["answer".into(); texts.len()])
        },
    );
    assert_eq!(sizes, [8, 4]);
    assert_eq!(
        described(&events),
        [
            "reading",
            "blocks 3",
            "translated 0",
            "translated 1",
            "translated 2",
            "done"
        ]
    );
}

#[test]
fn local_failure_skips_remaining_block_pieces_and_retries_a_repeated_later_block() {
    let long = format!(
        "{}。{}。{}。",
        "一".repeat(60),
        "二".repeat(60),
        "三".repeat(60)
    );
    let mut calls = 0;
    let (_, requests) = run(
        Some(vec![line(10, &long), line(110, "确定"), line(210, "确定")]),
        &mut TranslationCache::default(),
        &AtomicBool::new(false),
        |piece, _| {
            calls += 1;
            if calls <= 2 {
                Err(translation_failed())
            } else {
                Ok(format!("en:{piece}"))
            }
        },
        |_, _| {},
    );
    assert_eq!(
        requests,
        [
            format!("{}。", "一".repeat(60)),
            "确定".into(),
            "确定".into()
        ]
    );
}

#[test]
fn cancelling_after_a_cached_block_sends_no_later_local_request() {
    let mut cache = TranslationCache::default();
    run(
        Some(vec![line(10, "确定")]),
        &mut cache,
        &AtomicBool::new(false),
        |_, _| Ok("OK".into()),
        |_, _| {},
    );
    let (events, requests) = run(
        Some(vec![line(10, "确定"), line(110, "取消")]),
        &mut cache,
        &AtomicBool::new(false),
        |_, _| panic!("cancel must precede the next request"),
        |event, cancel| {
            if matches!(event, ScreenEvent::Translated { .. }) {
                cancel.store(true, Ordering::Release);
            }
        },
    );
    assert!(requests.is_empty());
    assert_eq!(described(&events), ["reading", "blocks 2", "translated 0"]);
}
