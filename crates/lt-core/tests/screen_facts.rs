use lt_core::screen::{
    filters::*,
    geom::{LineGeometry, PointF, RectF},
    line::*,
};

fn line(text: &str, width: f64, height: f64) -> Line {
    Line::new(text, RectF::new(0.0, 0.0, width, height))
}

fn detected(text: &str, width: f64, height: f64) -> Line {
    let mut line = Line::new(text, RectF::new(100.0, 100.0, width, height));
    line.layout_script = layout_script_for(text);
    line.layout_bounds = line.bounds;
    line.layout_glyph_height = layout_glyph_height_for(line.layout_script, line.bounds, text);
    line
}

#[test]
fn character_categories_match_the_utf16_recording() {
    let expected = include_bytes!("fixtures/screen/facts/character-flags.bin");
    let mismatches: Vec<_> = expected
        .iter()
        .enumerate()
        .filter_map(|(unit, expected)| {
            let unit = unit as u16;
            let actual = u8::from(is_letter(unit))
                | (u8::from(is_digit(unit)) << 1)
                | (u8::from(is_whitespace(unit)) << 2);
            (actual != *expected).then_some((unit, *expected, actual))
        })
        .collect();
    assert!(
        mismatches.is_empty(),
        "category differences: {mismatches:x?}"
    );
}

#[test]
fn every_latin_fold_matches_the_recorded_decomposition() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/screen/facts/latin-fold.json")).unwrap();
    for case in cases {
        let input = case["input"].as_str().unwrap();
        assert_eq!(
            fold_latin_diacritics(input),
            case["expected"].as_str().unwrap(),
            "{input}"
        );
    }
}

#[test]
fn script_is_read_off_the_text_itself() {
    for (text, expected) in [
        ("Shots deal more damage", LayoutScript::Latin),
        ("Lv100", LayoutScript::Latin),
        ("2026/07/24 23:29", LayoutScript::Latin),
        ("彈匣內每發子彈", LayoutScript::Cjk),
        ("カタカナ", LayoutScript::Cjk),
        ("攻", LayoutScript::Cjk),
        ("→", LayoutScript::Unknown),
        ("", LayoutScript::Unknown),
    ] {
        assert_eq!(layout_script_for(text), expected, "{text}");
    }
}

#[test]
fn mixed_script_blocks_are_classified_as_mixed() {
    for text in [
        "甲Glossaries",
        "文A日本語",
        "本Wikiについて",
        "2005 年から CSS, HTML, JavaScript のドキュメントを作成しています。",
    ] {
        assert_eq!(layout_script_for(text), LayoutScript::Mixed);
    }
}

#[test]
fn layout_script_is_chosen_per_block_regardless_of_render_normalization() {
    let blocks = || {
        ["OPTIONS", "ゲーム設定", "甲Glossaries", "文A日本語", "攻"]
            .iter()
            .map(|text| line(text, 240.0, 30.0))
            .collect()
    };
    let expected = [
        LayoutScript::Latin,
        LayoutScript::Cjk,
        LayoutScript::Mixed,
        LayoutScript::Mixed,
        LayoutScript::Cjk,
    ];
    for normalized in [
        normalize_blocks(blocks(), false),
        normalize_blocks(blocks(), true),
        normalize_automatic_blocks(blocks()),
    ] {
        assert_eq!(
            normalized
                .iter()
                .map(|b| b.layout_script)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn mixed_script_classification_and_text_are_independent_of_render_normalization() {
    for text in ["甲Glossaries", "文A日本語", "本Wikiについて"] {
        for normalized in [
            normalize_blocks(vec![line(text, 240.0, 30.0)], false),
            normalize_blocks(vec![line(text, 240.0, 30.0)], true),
            normalize_automatic_blocks(vec![line(text, 240.0, 30.0)]),
        ] {
            assert_eq!(normalized.len(), 1);
            assert_eq!(normalized[0].text, text);
            assert_eq!(normalized[0].layout_script, LayoutScript::Mixed);
        }
    }
}

#[test]
fn lone_ideograph_cleanup_runs_before_normalization() {
    let blocks = || vec![line("甲Glossaries", 240.0, 30.0)];
    for normalized in [
        normalize_blocks(blocks(), false),
        normalize_blocks(blocks(), true),
        normalize_automatic_blocks(blocks()),
    ] {
        assert_eq!(normalized[0].text, "甲Glossaries");
    }
    assert_eq!(strip_icon_ideographs(blocks())[0].text, "Glossaries");
}

#[test]
fn boxes_too_wide_for_their_characters_are_rejected() {
    for (text, width, height) in [
        ("□", 300.0, 40.0),
        ("□2", 264.0, 38.0),
        ("[", 120.0, 20.0),
        ("[0]2|", 500.0, 30.0),
    ] {
        assert!(is_too_wide_for_its_text(&detected(text, width, height)));
    }
}

#[test]
fn real_short_text_is_kept_by_the_shape_filter() {
    for (text, width, height) in [
        ("知らせ", 120.0, 40.0),
        ("お知らせ", 160.0, 40.0),
        ("仲町あられ", 200.0, 40.0),
        ("이벤트", 130.0, 42.0),
        ("YA", 135.0, 95.0),
        ("Yay!", 90.0, 60.0),
        ("Marina-san, are you okay?", 487.0, 59.0),
    ] {
        assert!(
            !is_too_wide_for_its_text(&detected(text, width, height)),
            "{text}"
        );
    }
}

#[test]
fn shape_filter_ignores_missing_height_and_does_not_count_whitespace() {
    assert!(!is_too_wide_for_its_text(&detected("□", 300.0, 0.0)));
    assert!(is_too_wide_for_its_text(&detected("  □  ", 300.0, 40.0)));
}

#[test]
fn two_letter_lines_use_the_box_instead_of_an_overblown_estimate() {
    assert_eq!(short_text_glyph_height(78.0, 40.0, 95.0, 2).0, 47.5);
}

#[test]
fn long_lines_keep_the_pitch_corrected_estimate() {
    assert_eq!(short_text_glyph_height(45.5, 300.0, 86.0, 16).0, 45.5);
}

#[test]
fn short_text_correction_only_makes_text_smaller_and_keeps_missing_boxes() {
    assert_eq!(short_text_glyph_height(20.0, 60.0, 60.0, 2).0, 20.0);
    assert_eq!(short_text_glyph_height(30.0, 40.0, 0.0, 1).0, 30.0);
}

#[test]
fn short_text_boundaries_match_the_pitch_clamp() {
    assert_eq!(short_text_glyph_height(60.0, 500.0, 60.0, 3).0, 30.0);
    assert_eq!(short_text_glyph_height(60.0, 500.0, 60.0, 4).0, 60.0);
    assert_eq!(short_text_glyph_height(26.24, 55.0, 32.0, 4).0, 16.0);
    assert!(!pitch_clamp_applies(64.0, 32.0, 4));
    assert_eq!(short_text_glyph_height(26.24, 64.0, 32.0, 4).0, 16.0);
    assert!(pitch_clamp_applies(64.0001, 32.0, 4));
    assert_eq!(short_text_glyph_height(26.24, 64.0001, 32.0, 4).0, 26.24);
}

#[test]
fn short_text_correction_is_consulted_exactly_where_pitch_is_not() {
    for count in [1, 3, 4, 5, 16] {
        for width in [10.0, 40.0, 63.9, 64.0, 64.0001, 65.0, 200.0, 1000.0] {
            let correction = short_text_glyph_height(26.24, width, 32.0, count).1;
            assert_ne!(pitch_clamp_applies(width, 32.0, count), correction.applied);
        }
    }
}

fn grid() -> Vec<(&'static str, RectF)> {
    [
        ("cost.", 62.0, 31.0),
        ("cost.", 61.0, 31.0),
        ("cost.", 63.0, 31.0),
        (
            "continues to use PP-DocLayoutV3 for layout analysis",
            1737.0,
            33.0,
        ),
        ("YA", 40.0, 95.0),
        ("Wow", 60.0, 95.0),
        ("Hello there friend", 300.0, 86.0),
        ("abc", 200.0, 40.0),
        ("abcd", 200.0, 40.0),
        ("TOO.", 55.0, 32.0),
        ("日本語です", 80.0, 50.0),
        ("設定", 80.0, 40.0),
        ("日本語のテキストです", 400.0, 40.0),
        ("BanG Dream! アニメ", 300.0, 30.0),
        ("test", 4.0, 2.0),
        ("test", 40.0, 0.0),
    ]
    .into_iter()
    .map(|(text, width, height)| (text, RectF::new(0.0, 0.0, width, height)))
    .collect()
}

#[test]
fn glyph_height_estimates_match_the_grid_on_both_render_paths() {
    let number =
        |value: Option<f64>| value.map_or_else(|| "null".to_owned(), |v| format!("{v:.4}"));
    let bounds = |b: RectF| format!("{:.4},{:.4},{:.4},{:.4}", b.x, b.y, b.width, b.height);
    let mut dump = String::new();
    for (text, rect) in grid() {
        let script = layout_script_for(text);
        let layout = layout_glyph_height_for(script, rect, text);
        let latin = normalize_block(Line::new(text, rect), false, None);
        let cjk = normalize_block(Line::new(text, rect), true, None);
        dump.push_str(&format!(
            "{text} | {:.4}x{:.4} | {script:?} | layout={} | renderLatin={} {} | renderCjk={} {}\n",
            rect.width,
            rect.height,
            number(layout),
            number(latin.render_glyph_height),
            bounds(latin.bounds),
            number(cjk.render_glyph_height),
            bounds(cjk.bounds)
        ));
    }
    assert_eq!(
        dump,
        include_str!("fixtures/screen/facts/glyph-height-grid.txt")
    );
}

#[test]
fn width_boundary_preserves_the_recorded_glyph_height_step() {
    let widths = [
        58.0, 59.0, 60.0, 61.0, 61.5, 61.9, 61.99, 62.0, 62.0001, 62.01, 62.1, 62.5, 63.0, 64.0,
        65.0, 66.0,
    ];
    let heights: Vec<_> = widths
        .iter()
        .map(|width| {
            layout_glyph_height_for(
                LayoutScript::Latin,
                RectF::new(0.0, 0.0, *width, 31.0),
                "cost.",
            )
            .unwrap()
        })
        .collect();
    for (width, height) in widths.iter().zip(&heights) {
        if *width <= 62.0 {
            assert_eq!(*height, 15.5);
        }
    }
    assert!((heights[7] - heights[8]).abs() < heights[7] / 10.0);
}

#[test]
fn trace_separates_the_width_and_glyph_count_conditions() {
    let trace = layout_glyph_height_traced(
        LayoutScript::Latin,
        RectF::new(145.0, 648.0, 62.0, 31.0),
        "cost.",
    )
    .1;
    assert!(trace.has_enough_glyphs);
    assert!(!trace.is_wide_enough);
    assert_eq!(trace.width_minus_twice_height, 0.0);
    assert!(!trace.pitch_branch_entered);
    assert!(!trace.pitch_selected);
    assert!(trace.pitch_candidate.unwrap() < trace.box_estimate);
    assert!(trace.short_text_applied);
    assert!(trace.short_text_selected);
    assert_eq!(trace.source, GlyphHeightSource::ShortText);
}

#[test]
fn trace_reports_the_value_actually_returned_on_every_grid_row() {
    for (text, bounds) in grid() {
        let script = layout_script_for(text);
        let plain = layout_glyph_height_for(script, bounds, text);
        let (traced, trace) = layout_glyph_height_traced(script, bounds, text);
        assert_eq!(plain, traced);
        assert_eq!(plain, trace.result);
        assert_eq!(script, trace.script);
    }
}

#[test]
fn scripts_without_an_estimate_still_report_their_inputs() {
    let (height, trace) = layout_glyph_height_traced(
        LayoutScript::Mixed,
        RectF::new(0.0, 0.0, 300.0, 30.0),
        "BanG Dream! アニメ",
    );
    assert_eq!(height, None);
    assert_eq!(trace.result, None);
    assert_eq!(trace.source, GlyphHeightSource::None);
    assert_eq!(trace.script, LayoutScript::Mixed);
    assert_eq!(trace.glyph_count, 13);
}

#[test]
fn layout_bounds_and_heights_are_independent_of_render_path() {
    let rect = RectF::new(40.0, 120.0, 240.0, 30.0);
    let blocks = || {
        vec![
            Line::new("Shots deal more damage", rect),
            Line::new("彈匣內每發子彈的傷害", rect),
        ]
    };
    let latin = normalize_blocks(blocks(), false);
    let cjk = normalize_blocks(blocks(), true);
    let automatic = normalize_automatic_blocks(blocks());
    for normalized in [&latin, &cjk, &automatic] {
        for block in normalized {
            assert_eq!(block.layout_bounds, rect);
        }
    }
    assert_ne!(latin[1].bounds.height, cjk[1].bounds.height);
    assert_eq!(
        latin
            .iter()
            .map(|b| b.layout_glyph_height)
            .collect::<Vec<_>>(),
        cjk.iter()
            .map(|b| b.layout_glyph_height)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        latin
            .iter()
            .map(|b| b.layout_glyph_height)
            .collect::<Vec<_>>(),
        automatic
            .iter()
            .map(|b| b.layout_glyph_height)
            .collect::<Vec<_>>()
    );
    assert_ne!(
        latin[0].render_glyph_height,
        automatic[0].render_glyph_height
    );
    let rect = RectF::new(0.0, 0.0, 120.0, 30.0);
    let cjk = normalize_block(Line::new("設定設定", rect), true, None);
    assert_eq!(cjk.layout_bounds, rect);
    assert!(cjk.bounds.height < rect.height);
    assert!(cjk.layout_glyph_height.unwrap() < cjk.layout_bounds.height);
    assert!((cjk.layout_glyph_height.unwrap() - cjk.bounds.height).abs() < 1e-9);
}

#[test]
fn mixed_and_unknown_blocks_have_no_layout_glyph_height() {
    for text in ["甲Glossaries", "→"] {
        assert_eq!(
            normalize_block(line(text, 240.0, 30.0), false, None).layout_glyph_height,
            None
        );
    }
}

#[test]
fn shape_verdict_uses_detection_geometry_on_every_render_path() {
    let blocks = || vec![Line::new("闇", RectF::new(10.0, 10.0, 74.0, 40.0))];
    let cjk = normalize_blocks(blocks(), true);
    assert_ne!(cjk[0].bounds, cjk[0].layout_bounds);
    assert!(cjk[0].bounds.width / cjk[0].bounds.height >= MAX_WIDTH_PER_GLYPH);
    assert!(cjk[0].layout_bounds.width / cjk[0].layout_bounds.height < MAX_WIDTH_PER_GLYPH);
    for normalized in [
        normalize_blocks(blocks(), false),
        cjk,
        normalize_automatic_blocks(blocks()),
    ] {
        assert!(!is_too_wide_for_its_text(&normalized[0]));
        assert_eq!(remove_misshapen_blocks(normalized).len(), 1);
    }
    let blocks = || vec![Line::new("□", RectF::new(10.0, 10.0, 300.0, 40.0))];
    for normalized in [
        normalize_blocks(blocks(), false),
        normalize_blocks(blocks(), true),
        normalize_automatic_blocks(blocks()),
    ] {
        assert!(is_too_wide_for_its_text(&normalized[0]));
        assert!(remove_misshapen_blocks(normalized).is_empty());
    }
}

#[test]
fn automatic_render_metrics_are_chosen_per_recognized_block() {
    for (text, expected) in [
        ("Hello world", false),
        ("甲Glossaries", false),
        ("翻譯", true),
        ("2026年5月8日", true),
        ("日本語", true),
        ("カタカナ", true),
        ("English 中文", true),
    ] {
        assert_eq!(uses_cjk_render_metrics_for_text(text), expected);
    }
    let normalized = normalize_automatic_blocks(
        ["Settings", "設定", "白"]
            .iter()
            .map(|s| line(s, 120.0, 20.0))
            .collect(),
    );
    assert_eq!(normalized.len(), 3);
    assert_eq!(normalized[0].text, "Settings");
    assert!(normalized[0].render_glyph_height.is_some());
    assert_eq!(normalized[0].bounds.height, 20.0);
    assert_eq!(normalized[1].text, "設定");
    assert!(normalized[1].render_glyph_height.is_none());
    assert!(normalized[1].bounds.height < 20.0);
    assert_eq!(normalized[2].text, "白");
    assert!(normalized[2].render_glyph_height.is_some());
    assert_eq!(normalized[2].layout_script, LayoutScript::Cjk);
    let normalized = normalize_automatic_blocks(
        ["Settings", "設定設定設定設定"]
            .iter()
            .map(|s| line(s, 120.0, 40.0))
            .collect(),
    );
    assert!(normalized[0].render_glyph_height.is_some());
    assert!(
        (normalized[1].bounds.height - normalized[0].render_glyph_height.unwrap()).abs() < 1e-6
    );
}

#[test]
fn icon_ideographs_are_removed_only_when_the_remainder_has_no_cjk() {
    for (input, expected) in [
        ("白", "白"),
        ("品", "品"),
        ("攻", "攻"),
        ("防", "防"),
        ("技", "技"),
        ("火", "火"),
        ("水", "水"),
        ("甲Glossaries", "Glossaries"),
        ("业spoken terms", "spoken terms"),
        ("文A English (US)", "A English (US)"),
        ("田Projects", "Projects"),
        ("日Wiki", "Wiki"),
        ("区Insights Settings", "Insights Settings"),
        ("三VOICE", "VOICE"),
        ("百LOG", "LOG"),
        ("CH是", "CH"),
        ("ev丘", "ev"),
        ("2026年5月8日", "2026年5月8日"),
        ("翻譯這個網頁", "翻譯這個網頁"),
        ("免費", "免費"),
        ("Google Translate", "Google Translate"),
        ("4.3 (82,985)·免費·參考資源", "4.3 (82,985)·免費·參考資源"),
        ("本Wikiについて", "本Wikiについて"),
        ("文A日本語", "文A日本語"),
    ] {
        assert_eq!(strip_lone_ideographs(input), expected);
    }
    assert_eq!(strip_lone_ideographs("閣Lv100 50"), "Lv100 50");
    let blocks = ["田Projects", "本Wikiについて", "攻", "Settings"]
        .iter()
        .map(|s| line(s, 120.0, 20.0))
        .collect();
    let cleaned = strip_icon_ideographs(blocks);
    assert_eq!(
        cleaned.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
        ["Projects", "本Wikiについて", "攻", "Settings"]
    );
    assert_eq!(cleaned.len(), 4);
}

#[test]
fn latin_accents_fold_without_changing_other_writing() {
    for (input, expected) in [
        ("That kind of șong", "That kind of song"),
        ("Ｃafé", "Ｃafe"),
        ("naïve", "naive"),
        ("Ārigatō", "Arigato"),
        ("That kind of song", "That kind of song"),
        ("翻譯這個網頁", "翻譯這個網頁"),
        ("2026年5月8日", "2026年5月8日"),
        ("がっこう", "がっこう"),
        ("ポケット", "ポケット"),
        ("月島まりな", "月島まりな"),
    ] {
        assert_eq!(fold_latin_diacritics(input), expected);
    }
}

#[test]
fn empty_and_touching_geometry_keep_distinct_meanings() {
    let zero = RectF::default();
    assert!(!zero.is_empty());
    assert!(RectF::EMPTY.is_empty());
    assert_eq!(RectF::EMPTY.union(zero), zero);
    let a = RectF::new(0.0, 0.0, 10.0, 10.0);
    let b = RectF::new(10.0, 5.0, 10.0, 10.0);
    assert!(a.intersects_with(b));
    assert_eq!(a.intersect(b), RectF::new(10.0, 5.0, 0.0, 5.0));
    assert!(!a.intersect(b).is_empty());
    assert!(a.intersect(b.offset(1.0, 0.0)).is_empty());
    assert!(a.contains(PointF::new(10.0, 10.0)));
    assert!(!a.contains_rect(RectF::EMPTY));
    assert!(a.inflate(-6.0, 0.0).is_empty());
}

#[test]
fn glyphs_are_counted_as_utf16_units() {
    assert_eq!(glyphs_in("A 😀 \u{20000}"), 5);
    assert_eq!(layout_script_for("\u{20000}"), LayoutScript::Unknown);
    assert_eq!(layout_script_for("\u{0345}"), LayoutScript::Unknown);
    let quad = RectF::new(0.0, 0.0, 357.0, 33.0).corners();
    let geometry = LineGeometry::from_quad(&quad).unwrap();
    assert!((geometry.length - 357.0).abs() <= 1.0);
    assert!((geometry.thickness - 33.0).abs() <= 1.0);
    assert!(geometry.angle_degrees.abs() <= 0.5);
    assert!(!geometry.is_tilted());
}

fn raw(text: &str, bounds: RectF) -> RawLine {
    RawLine {
        text: text.into(),
        quad: bounds.corners().to_vec(),
        char_scores: vec![0.99; text.encode_utf16().count()],
        detector_score: Some(0.9),
    }
}

#[test]
fn the_same_raw_detections_keep_the_same_blocks_on_every_render_path() {
    let capture: Vec<_> = [
        ("Confirm", 40.0, 30.0, 96.0, 26.0),
        ("Back", 260.0, 30.0, 62.0, 26.0),
        ("The quick brown fox jumps over", 40.0, 90.0, 420.0, 28.0),
        ("the lazy dog.", 40.0, 126.0, 170.0, 28.0),
        ("Send", 40.0, 190.0, 74.0, 31.0),
        ("to", 128.0, 195.0, 30.0, 25.0),
        ("田Projects", 40.0, 250.0, 132.0, 30.0),
        ("本Wikiについて", 240.0, 250.0, 168.0, 30.0),
        ("攻", 460.0, 250.0, 34.0, 30.0),
        ("闇", 540.0, 245.0, 74.0, 40.0),
        ("ゲーム設定を変更します", 40.0, 320.0, 300.0, 30.0),
        ("設定はいつでも戻せます", 40.0, 358.0, 300.0, 30.0),
    ]
    .into_iter()
    .map(|(text, x, y, w, h)| raw(text, RectF::new(x, y, w, h)))
    .collect();
    let converted = || convert_blocks(&capture);
    let paths = [
        apply_block_filters(converted(), false, true),
        apply_block_filters(converted(), false, false),
        apply_block_filters(converted(), true, false),
        apply_block_filters(converted(), true, false),
    ];
    for actual in &paths {
        assert_eq!(actual.len(), paths[0].len());
    }
    assert!(paths[2].iter().any(|block| block.text == "闇"));
}

fn rotated_quad(length: f64, thickness: f64, degrees: f64) -> Vec<PointF> {
    let radians = degrees * std::f64::consts::PI / 180.0;
    let (cos, sin) = (radians.cos(), radians.sin());
    [
        (-length / 2.0, -thickness / 2.0),
        (length / 2.0, -thickness / 2.0),
        (length / 2.0, thickness / 2.0),
        (-length / 2.0, thickness / 2.0),
    ]
    .into_iter()
    .map(|(x, y)| {
        PointF::new(
            (400.0 + x * cos - y * sin).round_ties_even(),
            (300.0 + x * sin + y * cos).round_ties_even(),
        )
    })
    .collect()
}

fn normalized_quad(text: &str, length: f64, thickness: f64, degrees: f64, automatic: bool) -> Line {
    let mut raw = raw(text, RectF::default());
    raw.quad = rotated_quad(length, thickness, degrees);
    let mut result = apply_block_filters(convert_blocks(&[raw]), false, automatic);
    assert_eq!(result.len(), 1);
    result.remove(0)
}

#[test]
fn quadrilaterals_keep_their_length_and_thickness_when_turned() {
    let level = LineGeometry::from_quad(&rotated_quad(357.0, 33.0, 0.0)).unwrap();
    assert!((level.length - 357.0).abs() <= 1.0);
    assert!((level.thickness - 33.0).abs() <= 1.0);
    assert!(level.angle_degrees.abs() <= 0.5);
    assert!(!level.is_tilted());
    for degrees in [30.0, -30.0] {
        let geometry = LineGeometry::from_quad(&rotated_quad(357.0, 33.0, degrees)).unwrap();
        assert!((geometry.length - 357.0).abs() <= 1.0);
        assert!((geometry.thickness - 33.0).abs() <= 1.0);
        assert!((geometry.angle_degrees - degrees).abs() <= 0.5);
        assert!(geometry.is_tilted());
    }
    for (degrees, expected) in [(10.0, false), (20.0, true), (40.0, true), (60.0, false)] {
        assert_eq!(
            LineGeometry::from_quad(&rotated_quad(357.0, 33.0, degrees))
                .unwrap()
                .is_tilted(),
            expected
        );
    }
    assert!(LineGeometry::from_quad(&[]).is_none());
    assert!(LineGeometry::from_quad(&[
        PointF::new(0.0, 0.0),
        PointF::new(10.0, 0.0),
        PointF::new(10.0, 5.0)
    ])
    .is_none());
}

#[test]
fn tilted_lines_get_the_font_height_of_the_same_line_set_level() {
    const TEXT: &str = "I THINK THAT GUY'S A PLAYER TOO.";
    for automatic in [false, true] {
        let level = normalized_quad(TEXT, 357.0, 33.0, 0.0, automatic);
        let tilted = normalized_quad(TEXT, 357.0, 33.0, 30.0, automatic);
        assert!(level.font_glyph_height.is_some());
        assert!(tilted.font_glyph_height.is_some());
        assert!(
            (level.font_glyph_height.unwrap() - tilted.font_glyph_height.unwrap()).abs() <= 1.0
        );
        assert!(tilted.font_glyph_height.unwrap() < tilted.bounds.height / 5.0);
    }
}

#[test]
fn tilted_lines_keep_the_upright_height_for_coverage() {
    let block = normalized_quad("I THINK THAT GUY'S A PLAYER TOO.", 357.0, 33.0, 30.0, false);
    assert!(block.render_glyph_height.unwrap() > 100.0);
    assert!(block.bounds.width <= block.bounds.height * 2.0);
    assert!((block.bounds.height * 0.82 - block.render_glyph_height.unwrap()).abs() <= 0.01);
}

#[test]
fn level_narrow_lines_use_half_the_box_on_every_metric() {
    let block = normalized_quad("TOO.", 55.0, 32.0, 0.0, false);
    assert!((block.bounds.height * 0.5 - block.render_glyph_height.unwrap()).abs() <= 0.01);
    assert_eq!(block.render_glyph_height, block.font_glyph_height);
    assert_eq!(block.render_glyph_height, block.layout_glyph_height);
}

#[test]
fn slight_slants_and_short_blobs_keep_the_upright_font_estimate() {
    for degrees in [0.0, 10.0] {
        let block = normalized_quad(
            "I THINK THAT GUY'S A PLAYER TOO.",
            357.0,
            33.0,
            degrees,
            false,
        );
        assert_eq!(block.render_glyph_height, block.font_glyph_height);
    }
    let block = normalized_quad("OK", 35.0, 21.0, -17.0, false);
    assert_eq!(block.render_glyph_height, block.font_glyph_height);
}
