use lt_core::screen::{
    erase::{self, RemovalBlock},
    geom::LineGeometry,
    line::Line,
    tilt::{self, TiltedText},
    ImageView,
};
use serde_json::{json, Value};
use std::{borrow::Cow, fs::File, io::BufReader, path::PathBuf};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/screen/tilt")
}

fn normalized(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(normalized).collect()),
        Value::Object(object) => {
            let mut output = serde_json::Map::new();
            for (key, value) in object {
                let mut parts = key.split('_');
                let mut name = parts.next().unwrap().to_owned();
                for part in parts {
                    let mut chars = part.chars();
                    if let Some(first) = chars.next() {
                        name.extend(first.to_uppercase());
                        name.push_str(chars.as_str());
                    }
                }
                output.insert(name, normalized(value));
            }
            if output.contains_key("text") && output.contains_key("bounds") {
                output.entry("tilt").or_insert(Value::Null);
            }
            if output.contains_key("width")
                && output.contains_key("height")
                && !output.contains_key("x")
            {
                output.insert(
                    "isEmpty".into(),
                    json!(output["width"].as_f64().unwrap() < 0.0),
                );
            }
            if output.contains_key("angleDegrees") {
                let geometry: LineGeometry =
                    serde_json::from_value(Value::Object(output.clone())).unwrap();
                output.insert("isTilted".into(), json!(geometry.is_tilted()));
            }
            if output.contains_key("degrees")
                && output.contains_key("box")
                && output.contains_key("lineQuads")
            {
                let tilt: TiltedText =
                    serde_json::from_value(Value::Object(output.clone())).unwrap();
                output.insert("outline".into(), json!(tilt.outline()));
                output.insert("eraseQuads".into(), json!(tilt.erase_quads()));
                output.insert("sampleArea".into(), json!(tilt.sample_area()));
            }
            Value::Object(output)
        }
        other => other,
    }
}

fn compare(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            for (key, value) in expected {
                compare(
                    actual
                        .get(key)
                        .unwrap_or_else(|| panic!("{path}.{key}: missing")),
                    value,
                    &format!("{path}.{key}"),
                );
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len(), "{path}: length");
            for (i, (a, b)) in actual.iter().zip(expected).enumerate() {
                compare(a, b, &format!("{path}[{i}]"));
            }
        }
        (Value::Number(a), Value::Number(b)) => assert!(
            (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() <= 1e-10,
            "{path}: {a} != {b}"
        ),
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn image(name: &str) -> (u32, u32, Vec<u8>) {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(fixtures().join(name)).unwrap()));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    let channels = info.color_type.samples();
    let pixels = buffer[..info.buffer_size()]
        .chunks_exact(channels)
        .flat_map(|p| [p[2], p[1], p[0], if channels == 4 { p[3] } else { 255 }])
        .collect();
    (info.width, info.height, pixels)
}

fn check(recording: &str) {
    let case: Value = serde_json::from_str(recording).unwrap();
    for (index, call) in case["calls"].as_array().unwrap().iter().enumerate() {
        let path = format!("{}[{index}]", case["test"].as_str().unwrap());
        if call["op"] == "erase" {
            let (w, h, pixels) = image(call["input"].as_str().unwrap());
            let blocks: Vec<_> = call["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| {
                    let shape: Option<TiltedText> =
                        serde_json::from_value(b["tilt"].clone()).unwrap();
                    RemovalBlock {
                        translated_text: b["translatedText"].as_str().unwrap().into(),
                        bounds: serde_json::from_value(b["bounds"].clone()).unwrap(),
                        source_line_bounds: serde_json::from_value(b["sourceLineBounds"].clone())
                            .unwrap(),
                        glyph_height: b["renderGlyphHeight"].as_f64(),
                        within: shape.as_ref().map(TiltedText::erase_quads),
                    }
                })
                .collect();
            let actual = erase::erase_text(&ImageView::new(w, h, w * 4, &pixels).unwrap(), &blocks);
            let (_, _, expected) = image(call["output"].as_str().unwrap());
            assert!(
                actual == expected,
                "{path}: repaired byte differences {}, first {:?}",
                actual.iter().zip(&expected).filter(|(a, b)| a != b).count(),
                actual.iter().zip(&expected).position(|(a, b)| a != b)
            );
            continue;
        }
        let input: Vec<Line> = serde_json::from_value(call["input"].clone()).unwrap();
        let actual = match call["op"].as_str().unwrap() {
            "straighten" => tilt::straighten(&input),
            "restore" => tilt::restore(&input),
            "level" => tilt::level(&input),
            other => panic!("Unknown {other}"),
        };
        assert_eq!(
            matches!(actual, Cow::Borrowed(_)),
            call["borrowed"].as_bool().unwrap(),
            "{path}: unchanged input identity"
        );
        compare(&normalized(json!(actual.as_ref())), &call["output"], &path);
    }
}

macro_rules! cases {
    ($($name:ident),* $(,)?)=>{$(#[test]fn $name(){check(include_str!(concat!("fixtures/screen/tilt/",stringify!($name),".json")));})*};
}

cases! {
    agroup_put_back_level_is_measured_against_in_its_upright_box_0,
    alevel_page_comes_back_as_it_went_0,
    alevel_page_is_not_touched_0,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_0,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_1,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_2,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_3,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_4,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_5,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_6,
    alone_line_is_drawn_tilted_only_when_it_is_long_and_tilted_enough_7,
    arotated_paragraph_is_one_group_of_its_lines_and_keeps_the_sound_effect_out_0,
    arotated_paragraph_is_one_group_of_its_lines_and_keeps_the_sound_effect_out_1,
    asheard_card_separates_its_comments_and_keeps_each_whole_0,
    atilted_line_that_touches_aneighbour_less_than_its_upright_box_would_keeps_its_tilt_0,
    atilted_paragraph_that_runs_into_the_level_line_above_is_drawn_level_0,
    aturned_card_is_drawn_rotated_and_acard_in_perspective_sheared_0,
    lines_that_are_not_one_tilted_card_are_left_alone_0,
    lines_that_are_not_one_tilted_card_are_left_alone_1,
    the_box_starts_at_the_cards_margin_rather_than_at_aline_that_overshoots_it_0,
    the_erase_stays_inside_the_lines_quadrilaterals_0,
    tilted_columns_are_levelled_only_where_standing_them_up_would_help_0,
    tilted_groups_that_meet_nothing_are_left_as_they_are_0,
}
