//! The screen job's event order, cancelling and cache, with a fake reader and translator.

use lt_core::{
    engines::{TextLine, TextRecognizer},
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
            requests.push(piece.to_owned());
            translate(piece, cancel)
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
            &mut |_| Ok("Hello".into()),
            &AtomicBool::new(false),
            &mut TranslationCache::default(),
            JobOutput {
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
