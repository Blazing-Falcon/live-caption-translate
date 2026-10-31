use lt_core::screen::{
    detection::Detection,
    geom::LineGeometry,
    line::Line,
    tilt::TiltedText,
    vertical::{
        columns_first, detection, geometry, grouping, probability::DetectorProbability, repeated,
        ruby, tilted,
    },
    ImageView,
};
use serde_json::{json, Value};
use std::{fs::File, io::BufReader, path::PathBuf};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/screen/vertical")
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
            if output.contains_key("x") && output.contains_key("y") && !output.contains_key("width")
            {
                output.remove("isEmpty");
                output.remove("length");
                output.remove("lengthSquared");
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
            (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() <= 1e-6,
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

fn lines(value: &Value) -> Vec<Line> {
    serde_json::from_value(value.clone()).unwrap()
}

#[test]
fn turned_pixels_and_rectangle_mapping_agree() {
    use lt_core::screen::{geom::RectF, vertical};
    let mut pixels = vec![0; 80 * 200 * 4];
    for y in 30..39 {
        for x in 10..16 {
            pixels[(y * 80 + x) * 4..][..4].copy_from_slice(&[0, 0, 255, 255]);
        }
    }
    let turned = vertical::turn(&ImageView::new(80, 200, 320, &pixels).unwrap());
    assert_eq!((turned.width, turned.height), (200, 80));
    let mut found = RectF::EMPTY;
    for y in 0..turned.height {
        for x in 0..turned.width {
            if turned.view().row(y)[x as usize * 4 + 2] == 255 {
                found = found.union(RectF::new(f64::from(x), f64::from(y), 1.0, 1.0));
            }
        }
    }
    assert_eq!(
        geometry::to_upright(found, 80),
        RectF::new(10.0, 30.0, 6.0, 9.0)
    );
}

#[test]
fn column_crop_keeps_top_to_bottom_marks_in_reading_order() {
    use lt_core::screen::raster;
    let mut pixels = vec![255; 30 * 90 * 4];
    for y in 10..70 {
        for x in 5..25 {
            pixels[(y * 30 + x) * 4..][..4].copy_from_slice(if y < 40 {
                &[0, 0, 255, 255]
            } else {
                &[255, 0, 0, 255]
            });
        }
    }
    let points = [(5, 10), (25, 10), (25, 70), (5, 70)];
    let crop = raster::crop(
        &ImageView::new(30, 90, 120, &pixels).unwrap(),
        &geometry::for_recognition(points),
    );
    assert_eq!((crop.width, crop.height), (60, 20));
    assert_eq!(&crop.view().row(10)[40..44], &[0, 0, 255, 255]);
    assert_eq!(&crop.view().row(10)[180..184], &[255, 0, 0, 255]);
}
fn detection(value: &Value) -> Detection {
    let points: Vec<_> = value["boxPoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["x"].as_i64().unwrap() as i32,
                p["y"].as_i64().unwrap() as i32,
            )
        })
        .collect();
    Detection {
        points: points.try_into().unwrap(),
        score: value["score"].as_f64().unwrap() as f32,
    }
}
fn box_value(b: &Detection) -> Value {
    json!({"boxPoints":b.points.map(|(x,y)|json!({"x":x,"y":y})),"score":b.score})
}
fn check(recording: &str) {
    let case: Value = serde_json::from_str(recording).unwrap();
    for (index, call) in case["calls"].as_array().unwrap().iter().enumerate() {
        let i = &call["input"];
        let pixel_input = i["image"].as_str().map(image);
        let pixels = pixel_input
            .as_ref()
            .map(|(w, h, p)| ImageView::new(*w, *h, *w * 4, p).unwrap());
        let actual = match call["op"].as_str().unwrap() {
            "prepare" => json!(geometry::prepare_blocks(lines(i))),
            "glyph_pitch" => json!(geometry::glyph_pitch(&lines(i)[0])),
            "for_recognition" => box_value(&Detection {
                points: geometry::for_recognition(detection(i).points),
                score: detection(i).score,
            }),
            "join" => json!(geometry::join_column_fragments(
                lines(&i["blocks"]),
                pixels.as_ref()
            )),
            "group" => json!(grouping::group(
                lines(&i["blocks"]),
                pixels.as_ref(),
                i["language"].as_str().map(|l| l == "JA" || l == "ZH")
            )),
            "merge" => json!(grouping::merge_columns(lines(i))),
            "runs_across" => json!(grouping::runs_across(&lines(i)[0])),
            "without_ruby" => json!(grouping::without_ruby(lines(i))),
            "without_rows" => json!(grouping::without_rows_over_columns(
                lines(&i["blocks"]),
                &lines(&i["columns"])
            )),
            "wordless" => json!(grouping::without_wordless_groups(lines(i))),
            "repeated" => json!(repeated::drop_repeated(lines(i))),
            "separate" => {
                let (writing, readings) = ruby::separate(lines(i));
                json!({"writing":writing,"readings":readings})
            }
            "angle" => json!(tilted::angle(
                &serde_json::from_value::<Vec<LineGeometry>>(i["columns"].clone()).unwrap(),
                i["trustedLength"].as_f64().unwrap()
            )),
            "quad" => json!(tilted::quad(
                serde_json::from_value(i["box"].clone()).unwrap(),
                serde_json::from_value(i["line"].clone()).unwrap()
            )),
            "upright_rect" => json!(geometry::to_upright(
                serde_json::from_value(i["box"].clone()).unwrap(),
                i["sourceWidth"].as_u64().unwrap() as u32
            )),
            "upright_block" => json!([geometry::block_to_upright(
                lines(&i["blocks"])[0].clone(),
                i["sourceWidth"].as_u64().unwrap() as u32
            )]),
            "missed" => json!(repeated::pieces_the_upright_pass_missed(
                &serde_json::from_value::<Vec<_>>(i["upright"].clone()).unwrap(),
                &serde_json::from_value::<Vec<_>>(i["turned"].clone()).unwrap(),
                i["sourceWidth"].as_u64().unwrap() as u32
            )),
            "worth" => json!(repeated::worth_keeping(&lines(i)[0])),
            "says_read" => json!(repeated::says_what_was_already_read(
                &lines(&i["candidate"])[0],
                &lines(&i["upright"])
            )),
            "carve" => {
                let mut map: Vec<f32> = serde_json::from_value(i["map"].clone()).unwrap();
                let changed = columns_first::carve(
                    &mut map,
                    i["width"].as_u64().unwrap() as usize,
                    i["height"].as_u64().unwrap() as usize,
                    i["threshold"].as_f64().unwrap() as f32,
                );
                json!({"changed":changed,"map":map})
            }
            "extend" => box_value(&detection::extend(
                pixels.as_ref().unwrap(),
                &detection(&i["box"]),
            )),
            "split" => {
                let p = &i["probability"];
                let values: Vec<f32> = if p.is_null() {
                    Vec::new()
                } else {
                    serde_json::from_value(p["values"].clone()).unwrap()
                };
                let probability = (!p.is_null()).then(|| DetectorProbability {
                    values: &values,
                    width: p["width"].as_u64().unwrap() as usize,
                    height: p["height"].as_u64().unwrap() as usize,
                    scale_x: p["scaleX"].as_f64().unwrap() as f32,
                    scale_y: p["scaleY"].as_f64().unwrap() as f32,
                    threshold: p["threshold"].as_f64().unwrap() as f32,
                    score_threshold: p["scoreThreshold"].as_f64().unwrap() as f32,
                    unclip_ratio: p["unclipRatio"].as_f64().unwrap() as f32,
                });
                let boxes: Vec<_> = i["boxes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(detection)
                    .collect();
                json!(
                    detection::split(pixels.as_ref().unwrap(), &boxes, probability.as_ref())
                        .iter()
                        .map(box_value)
                        .collect::<Vec<_>>()
                )
            }
            other => panic!("unknown {other}"),
        };
        compare(
            &normalized(actual),
            &normalized(call["output"].clone()),
            &format!("{}[{index}]", case["test"].as_str().unwrap()),
        );
    }
}

macro_rules! cases {
($($name:ident),* $(,)?)=>{$(#[test]fn $name(){check(include_str!(concat!("fixtures/screen/vertical/",stringify!($name),".json")));})*};
}
cases! {
a_column_alone_is_tilted_only_when_long_and_clearly_turned_0,
a_column_alone_is_tilted_only_when_long_and_clearly_turned_1,
a_column_alone_is_tilted_only_when_long_and_clearly_turned_2,
a_column_alone_is_tilted_only_when_long_and_clearly_turned_3,
a_reduced_pictures_columns_can_be_held_to_more_than_two_to_one_0,
a_short_fit_does_not_count_and_the_long_column_is_judged_alone_0,
a_straight_page_carries_no_tilt_0,
a_tilted_group_is_grouped_exactly_as_a_straight_one_and_only_carries_its_tilt_0,
the_level_box_is_the_columns_own_room_and_the_erase_covers_them_end_to_end_0,
two_columns_are_tilted_when_they_agree_0,
two_columns_are_tilted_when_they_agree_1,
two_columns_are_tilted_when_they_agree_2,
two_columns_are_tilted_when_they_agree_3,
two_columns_are_tilted_when_they_agree_4,
two_columns_are_tilted_when_they_agree_5,
two_columns_are_tilted_when_they_agree_6,
a_column_stays_a_column_0,
a_column_stays_a_column_1,
a_column_stays_a_column_2,
a_level_caption_of_several_lines_is_one_sentence_0,
a_line_read_off_the_turned_frame_keeps_its_angle_on_the_upright_one_0,
a_line_turned_steeply_enough_to_have_a_tall_upright_box_still_runs_across_0,
a_row_the_upright_box_already_called_a_row_stays_one_0,
a_tilted_card_is_set_as_one_sentence_along_the_card_0,
the_columns_come_out_exactly_as_they_did_without_it_0,
the_word_on_a_line_of_its_own_is_a_line_and_not_a_column_0,
the_word_on_a_line_of_its_own_is_not_taken_for_a_second_reading_of_the_line_above_0,
without_a_language_the_rows_are_handed_on_line_by_line_as_before_0,
the_screenshot_flow_keeps_a_box_that_spans_the_frame_0,
a_box_over_artwork_is_left_alone_0,
a_box_that_already_holds_its_column_does_not_move_0,
a_box_that_cut_the_top_character_reaches_back_for_it_0,
a_rule_crossing_the_column_stops_the_reach_0,
ink_beside_the_column_with_a_gutter_between_is_not_a_rule_0,
a_balloon_above_another_is_not_joined_to_it_0,
a_balloon_set_corner_to_corner_with_another_is_not_joined_to_it_0,
a_column_read_in_two_pieces_is_not_cut_between_them_0,
a_first_column_set_higher_than_the_next_stays_in_its_block_0,
a_remark_stepped_down_past_the_one_before_it_is_its_own_block_0,
a_short_column_stepped_up_in_its_own_lobe_is_its_own_block_0,
big_lettering_beside_a_balloon_is_not_joined_to_it_0,
columns_running_the_same_length_are_one_balloon_0,
columns_stepping_down_in_one_balloon_are_one_group_0,
several_columns_of_one_remark_are_one_block_0,
the_ragged_columns_of_an_oval_balloon_are_one_block_0,
three_remarks_stepping_down_are_three_blocks_0,
a_head_whose_column_is_not_found_is_given_to_the_writing_under_it_0,
a_page_with_no_row_is_left_as_the_model_wrote_it_0,
a_row_standing_clear_of_the_columns_is_left_as_the_model_wrote_it_0,
heads_lit_as_one_row_go_back_to_their_columns_0,
tails_lit_as_one_row_go_back_to_their_columns_0,
the_readings_between_the_heads_are_taken_off_the_map_0,
a_part_does_not_keep_what_a_gutter_separates_from_its_ink_0,
a_part_that_runs_the_whole_quad_keeps_the_quads_edges_0,
a_quad_holding_one_column_is_left_alone_0,
a_short_part_is_not_given_the_height_of_the_column_beside_it_0,
a_column_already_boxed_elsewhere_is_left_to_that_box_0,
a_column_and_its_reading_are_not_cut_0,
a_cut_column_gets_the_box_the_library_would_have_drawn_0,
a_horizontal_line_is_not_cut_into_characters_0,
scribbled_artwork_beside_a_column_is_not_a_second_column_0,
the_reading_between_cut_columns_is_left_out_0,
two_columns_on_the_artwork_come_apart_0,
without_a_map_the_quad_on_artwork_is_left_as_it_was_0,
a_different_piece_of_writing_inside_another_box_is_kept_0,
a_reading_inside_the_writings_box_is_kept_0,
a_reading_inside_the_writings_box_is_kept_1,
a_reading_inside_the_writings_box_is_kept_2,
a_reading_inside_the_writings_box_is_kept_3,
a_second_detection_of_the_same_writing_goes_0,
the_copy_holding_less_of_the_writing_is_the_one_that_goes_0,
the_same_writing_framed_twice_is_read_once_0,
two_boxes_that_do_not_lie_on_each_other_are_both_kept_0,
a_row_just_above_the_columns_it_was_taken_from_is_dropped_0,
a_row_lying_on_a_balloon_is_dropped_0,
a_row_more_than_its_height_from_any_column_is_kept_0,
a_row_over_columns_read_in_pieces_is_dropped_0,
a_row_that_touches_no_column_is_kept_0,
a_row_under_a_short_wide_box_is_kept_0,
a_row_under_a_short_wide_column_is_kept_when_the_columns_are_asked_0,
rows_on_a_page_of_rows_are_kept_0,
a_candidate_holding_a_kanji_beside_kana_is_left_alone_0,
a_candidate_holding_a_kanji_is_set_aside_rather_than_dropped_0,
a_column_set_aside_that_says_more_than_one_character_is_drawn_0,
a_kana_column_beside_kana_is_kept_0,
a_narrow_column_of_dialogue_is_kept_0,
a_narrow_column_of_dialogue_is_kept_1,
a_narrow_column_of_dialogue_is_kept_2,
a_narrow_column_on_the_left_is_kept_0,
a_narrow_column_standing_away_from_the_writing_is_kept_0,
a_narrow_column_that_runs_past_the_writing_is_kept_0,
a_reading_beside_its_column_is_dropped_0,
a_row_beside_a_column_is_not_a_reading_0,
a_set_aside_column_leaves_its_room_to_the_writing_for_grouping_only_0,
a_single_character_set_aside_is_not_drawn_0,
the_room_a_reading_took_up_is_left_to_the_writing_0,
a_fragment_set_at_the_bodys_own_size_is_kept_0,
a_fragment_set_at_the_bodys_own_size_is_kept_1,
a_fragment_set_at_the_bodys_own_size_is_kept_2,
a_reading_over_a_horizontal_sign_is_dropped_0,
a_reading_sitting_on_its_text_is_dropped_0,
a_reading_that_barely_touches_its_text_is_kept_0,
a_smaller_title_line_of_a_name_plate_is_kept_0,
two_balloons_that_merely_sit_near_each_other_are_both_kept_0,
group_keeps_horizontal_text_and_marks_it_as_running_across_0,
group_leaves_column_groups_unmarked_0,
merge_columns_carries_layout_metrics_onto_the_group_0,
merge_columns_drops_wide_horizontal_text_before_it_bridges_separate_columns_0,
merge_columns_joins_columns_that_start_at_different_heights_0,
merge_columns_joins_right_to_left_and_keeps_other_groups_separate_0,
merge_columns_judges_the_column_shape_on_layout_bounds_0,
merge_columns_keeps_a_one_character_wide_detection_0,
merge_columns_keeps_the_box_width_when_the_area_would_overshoot_0,
merge_columns_sizes_the_cell_from_the_area_when_one_box_holds_two_columns_0,
merge_columns_still_refuses_columns_across_a_gutter_0,
the_screenshot_flow_keeps_a_short_unsure_reading_0,
a_group_holding_a_word_is_kept_0,
a_group_holding_a_word_is_kept_1,
a_group_holding_a_word_is_kept_2,
a_group_holding_a_word_is_kept_3,
a_group_holding_a_word_is_kept_4,
a_group_with_no_word_in_it_is_dropped_0,
a_group_with_no_word_in_it_is_dropped_1,
a_group_with_no_word_in_it_is_dropped_2,
a_group_with_no_word_in_it_is_dropped_3,
a_group_with_no_word_in_it_is_dropped_4,
a_group_with_no_word_in_it_is_dropped_5,
continuous_ink_and_slanted_quads_are_not_split_0,
divider_prevents_joining_across_balloon_boundary_0,
fragments_join_down_column_before_reading_next_column_0,
narrow_unassigned_ink_becomes_its_own_part_rather_than_taking_the_cut_away_0,
realtime_collapse_is_measured_across_columns_not_down_their_height_0,
recognition_crop_reads_top_to_bottom_left_to_right_without_changing_detection_quad_0,
short_crop_does_not_turn_upright_single_glyph_0,
split_merged_column_uses_blank_gutter_on_either_polarity_0,
split_merged_column_uses_blank_gutter_on_either_polarity_1,
vertical_metrics_keep_coverage_and_use_character_pitch_0,
wide_padding_does_not_bridge_separate_speakers_0,
a_column_the_upright_pass_read_in_fragments_is_not_taken_again_0,
a_mark_on_the_turned_frame_maps_back_to_where_it_started_0,
a_piece_the_upright_pass_already_read_is_not_taken_again_0,
a_piece_the_upright_pass_never_read_is_taken_0,
a_piece_under_the_box_of_a_column_that_was_read_as_something_else_is_taken_0,
a_reading_of_different_words_in_the_same_place_is_kept_0,
a_reading_that_repeats_what_was_already_read_is_dropped_0,
a_single_character_the_upright_pass_read_cannot_speak_for_a_sentence_0,
a_speck_read_inside_a_column_does_not_stand_for_the_column_0,
mapping_a_block_back_moves_its_layout_box_as_well_0,
only_a_piece_holding_more_than_one_glyph_is_added_0,
only_a_piece_holding_more_than_one_glyph_is_added_1,
only_a_piece_holding_more_than_one_glyph_is_added_2,
only_a_piece_holding_more_than_one_glyph_is_added_3,
only_a_piece_holding_more_than_one_glyph_is_added_4,
only_a_piece_holding_more_than_one_glyph_is_added_5,
the_turned_passs_own_boxes_are_weighed_against_each_other_too_0
}

cases! {
vertical_grouping_answers_on_geometry_not_on_a_capture_profile_0,
vertical_recognition_preserves_layout_bounds_as_well_as_bounds_0,
vertical_recognition_preserves_source_frame_and_requests_vertical_recognition_0,
vertical_text_layout_script_follows_actual_text_not_orientation_0,
vertical_text_layout_script_follows_actual_text_not_orientation_1,
vertical_text_layout_script_follows_actual_text_not_orientation_2
}
