use lt_core::{
    error::Error,
    screen::{
        erase::{self, RemovalBlock, TextRegion},
        geom::RectF,
        imaging::{ImageBuffer, PixelType},
        ImageView,
    },
};
use std::sync::atomic::AtomicBool;

fn paint(image: &ImageBuffer, area: (usize, usize, usize, usize), bgr: [u8; 3]) {
    for y in area.1..area.1 + area.3 {
        for x in area.0..area.0 + area.2 {
            for (c, v) in bgr.iter().enumerate() {
                image.set_byte(y, x * 3 + c, *v);
            }
        }
    }
}
fn bgra(image: &ImageBuffer) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.width * image.height * 4);
    for y in 0..image.height {
        for x in 0..image.width {
            bytes.extend([
                image.byte(y, x * 3),
                image.byte(y, x * 3 + 1),
                image.byte(y, x * 3 + 2),
                255,
            ]);
        }
    }
    bytes
}
fn region(bounds: RectF, glyph_height: Option<f64>) -> TextRegion {
    TextRegion {
        bounds,
        glyph_height,
        within: None,
    }
}
fn repair(source: &ImageBuffer, regions: &[TextRegion]) -> ImageBuffer {
    let mask = erase::build_mask(source, regions);
    erase::repair(source, &mask).image
}

#[test]
fn repair_removes_strokes_without_changing_source_or_untranslated_region() {
    let source = ImageBuffer::solid(213, 143, [60, 40, 20]);
    paint(&source, (42, 35, 3, 18), [255; 3]);
    paint(&source, (152, 95, 3, 18), [255; 3]);
    let pixels = bgra(&source);
    let frame = ImageView::new(213, 143, 213 * 4, &pixels).unwrap();
    let result = erase::erase_text(
        &frame,
        &[
            RemovalBlock {
                translated_text: "字".into(),
                bounds: RectF::new(38.0, 32.0, 14.0, 25.0),
                source_line_bounds: None,
                glyph_height: None,
                within: None,
            },
            RemovalBlock {
                translated_text: " ".into(),
                bounds: RectF::new(148.0, 92.0, 14.0, 25.0),
                source_line_bounds: None,
                glyph_height: None,
                within: None,
            },
        ],
    );
    assert_eq!(result.len(), pixels.len());
    assert_eq!(
        &pixels[(40 * 213 + 43) * 4..(40 * 213 + 43) * 4 + 3],
        &[255; 3]
    );
    let at = (40 * 213 + 43) * 4;
    assert!((15..=30).contains(&result[at + 2]));
    assert!((55..=70).contains(&result[at]));
    let corner = (142 * 213 + 212) * 4;
    assert_eq!(&result[corner..corner + 4], &[60, 40, 20, 255]);
    let untouched = (100 * 213 + 153) * 4;
    assert_eq!(&result[untouched..untouched + 4], &[255; 4]);
}
#[test]
fn repair_erases_the_fade_around_an_outlined_glyph_and_not_only_the_stroke() {
    let background = [120, 90, 60];
    let source = ImageBuffer::solid(320, 160, background);
    let (left, top, stroke, gap, height, strokes) = (40, 40, 8, 28, 40, 5);
    let (outline, fade) = (3, 5);
    let band = outline + fade;
    let columns: Vec<_> = (0..strokes).map(|i| left + i * (stroke + gap)).collect();
    for step in (1..=fade).rev() {
        let toward = (fade - step + 1) as f64 / (fade + 1) as f64;
        let color = background.map(|v| (f64::from(v) * (1.0 - toward)) as u8);
        let grow = outline + step;
        for x in &columns {
            paint(
                &source,
                (x - grow, top - grow, stroke + grow * 2, height + grow * 2),
                color,
            );
        }
    }
    for x in &columns {
        paint(
            &source,
            (
                x - outline,
                top - outline,
                stroke + outline * 2,
                height + outline * 2,
            ),
            [0; 3],
        );
        paint(&source, (*x, top, stroke, height), [255; 3]);
    }
    let result = repair(
        &source,
        &[region(
            RectF::new(
                columns[0] as f64,
                (top - height / 2) as f64,
                (columns.last().unwrap() + stroke - columns[0]) as f64,
                (height * 2) as f64,
            ),
            Some(height as f64),
        )],
    );
    for x in columns {
        for y in top - band..top + height + band {
            let apart: i32 = (0..3)
                .map(|c| {
                    (i32::from(result.byte(y, (x + stroke / 2) * 3 + c)) - i32::from(background[c]))
                        .abs()
                })
                .sum();
            assert!(apart <= 24, "({},{}): {apart}", x + stroke / 2, y);
        }
    }
}
#[test]
fn repair_erases_a_lone_stroke_the_dark_response_cannot_see() {
    let background = [26, 18, 18];
    let source = ImageBuffer::solid(360, 120, background);
    let (top, height, stroke, gap) = (40, 40, 3, 14);
    for i in 0..8 {
        paint(
            &source,
            (40 + i * (stroke + gap), top, stroke, height),
            [255; 3],
        );
    }
    let alone = 260;
    paint(&source, (alone, top, stroke, height), [255; 3]);
    let result = repair(
        &source,
        &[region(
            RectF::new(30.0, (top - 4) as f64, 240.0, (height + 8) as f64),
            Some(height as f64),
        )],
    );
    for y in top..top + height {
        let apart: i32 = (0..3)
            .map(|c| {
                (i32::from(result.byte(y, (alone + 1) * 3 + c)) - i32::from(background[c])).abs()
            })
            .sum();
        assert!(apart <= 24, "({},{}): {apart}", alone + 1, y);
    }
}
#[test]
fn repair_erases_the_middle_of_a_stroke_wider_than_the_widest_element() {
    let source = ImageBuffer::solid(260, 200, [255; 3]);
    paint(&source, (70, 40, 34, 120), [0; 3]);
    paint(&source, (50, 84, 150, 32), [0; 3]);
    let result = repair(
        &source,
        &[region(RectF::new(44.0, 36.0, 162.0, 128.0), Some(120.0))],
    );
    for (x, y) in [(87, 60), (87, 100), (150, 100), (87, 140)] {
        assert!(
            result.byte(y, x * 3 + 2) > 200,
            "({x},{y}) left at {}",
            result.byte(y, x * 3 + 2)
        );
    }
}
#[test]
fn repair_fills_a_tile_whose_whole_neighbourhood_is_masked() {
    let source = ImageBuffer::solid(400, 400, [80, 60, 40]);
    for x in (0..400).step_by(8) {
        paint(&source, (x, 0, 4, 400), [20, 10, 0]);
    }
    let mask = ImageBuffer::new(400, 400, PixelType::U8C1);
    paint(&source, (20, 50, 360, 300), [255; 3]);
    mask.view(20, 50, 360, 300).set_to(255.0, None);
    let repaired = erase::repair(&source, &mask);
    assert!((0..=120).contains(&repaired.image.byte(144, 144 * 3)));
    assert!((0..=80).contains(&repaired.image.byte(144, 144 * 3 + 2)));
}
#[test]
fn empty_translation_returns_independent_unchanged_frame() {
    let mut pixels = vec![0; 7 * 5 * 4];
    pixels[(4 * 7 + 6) * 4 + 2] = 255;
    let frame = ImageView::new(7, 5, 7 * 4, &pixels).unwrap();
    let mut result = erase::erase_text(&frame, &[]);
    assert_eq!(result, pixels);
    result[0] = 255;
    assert_eq!(pixels[0], 0);
    assert_eq!(result[(4 * 7 + 6) * 4 + 2], 255);
}
#[test]
fn repair_honors_cancellation_before_pixel_conversion() {
    let pixels = vec![0; 7 * 5 * 4];
    let frame = ImageView::new(7, 5, 7 * 4, &pixels).unwrap();
    assert!(matches!(
        erase::erase_text_cancellable(&frame, &[], &AtomicBool::new(true)),
        Err(Error::Stopped)
    ));
}

#[test]
fn an_empty_source_line_list_uses_block_bounds_and_repair_makes_capture_pixels_opaque() {
    let source = ImageBuffer::solid(213, 143, [60, 40, 20]);
    paint(&source, (42, 35, 3, 18), [255; 3]);
    let mut pixels = bgra(&source);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 0;
    }
    let frame = ImageView::new(213, 143, 213 * 4, &pixels).unwrap();
    let result = erase::erase_text(
        &frame,
        &[RemovalBlock {
            translated_text: "字".into(),
            bounds: RectF::new(38.0, 32.0, 14.0, 25.0),
            source_line_bounds: Some(Vec::new()),
            glyph_height: None,
            within: None,
        }],
    );
    assert!(result
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[3] == 255));
    assert!((15..=30).contains(&result[(40 * 213 + 43) * 4 + 2]));
    assert!((55..=70).contains(&result[(40 * 213 + 43) * 4]));
    assert!(pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0));
}
