use lt_core::screen::{
    geom::LineGeometry,
    group::{engine, next_line, profile::GroupingProfile, ruby, speaker},
    line::Line,
};
use serde_json::{json, Value};

fn normalize(value: Value) -> Value {
    match value {
        Value::Array(array) => Value::Array(array.into_iter().map(normalize).collect()),
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
                output.insert(name, normalize(value));
            }
            if output.contains_key("text") && output.contains_key("bounds") {
                let lines = output
                    .get("sourceLineBounds")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!([output["bounds"].clone()]));
                output.insert("lines".into(), lines);
                output.entry("tilt").or_insert(Value::Null);
            }
            if output.contains_key("length")
                && output.contains_key("thickness")
                && output.contains_key("angleDegrees")
            {
                let geometry: LineGeometry =
                    serde_json::from_value(Value::Object(output.clone())).unwrap();
                output.insert("isTilted".into(), json!(geometry.is_tilted()));
            }
            Value::Object(output)
        }
        other => other,
    }
}

fn compare(actual: &Value, expected: &Value, path: &str) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            for (key, value) in expected {
                compare(
                    actual
                        .get(key)
                        .unwrap_or_else(|| panic!("{path}.{key}: missing field")),
                    value,
                    &format!("{path}.{key}"),
                );
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            assert_eq!(actual.len(), expected.len(), "{path}: length");
            for (i, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                compare(actual, expected, &format!("{path}[{i}]"));
            }
        }
        (Value::Number(expected), Value::Number(actual)) => {
            let a = actual.as_f64().unwrap();
            let b = expected.as_f64().unwrap();
            assert!((a - b).abs() <= 1e-10, "{path}: {a} != {b}");
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn check(recording: &str) {
    let case: Value = serde_json::from_str(recording).unwrap();
    for (index, call) in case["calls"].as_array().unwrap().iter().enumerate() {
        let path = format!("{}[{index}]", case["test"].as_str().unwrap());
        let kind = call["kind"].as_str().unwrap();
        if kind == "speaker" {
            assert_eq!(
                speaker::opens(call["input"].as_str().unwrap()),
                call["output"].as_bool().unwrap()
            );
            continue;
        }
        if kind == "inline_text" {
            assert_eq!(
                engine::join_inline_text(call["a"].as_str().unwrap(), call["b"].as_str().unwrap()),
                call["output"].as_str().unwrap()
            );
            continue;
        }
        let input: Vec<Line> = serde_json::from_value(call["input"].clone()).unwrap();
        match kind {
            "group" => {
                let profile: GroupingProfile =
                    serde_json::from_value(call["profile"].clone()).unwrap();
                let report = engine::group_with_profile(&input, &profile, true);
                compare(
                    &normalize(serde_json::to_value(&report.groups).unwrap()),
                    &call["output"],
                    &path,
                );
                compare(
                    &normalize(serde_json::to_value(&report.decisions).unwrap()),
                    &call["decisions"],
                    &format!("{path}.decisions"),
                );
                compare(
                    &normalize(serde_json::to_value(&report.trace).unwrap()),
                    &call["trace"],
                    &format!("{path}.trace"),
                );
                let plain = engine::group_with_profile(&input, &profile, false);
                assert_eq!(
                    plain.groups, report.groups,
                    "{path}: tracing changes grouping"
                );
            }
            "build_group" => compare(
                &normalize(serde_json::to_value(engine::build_group(&input)).unwrap()),
                &call["output"],
                &path,
            ),
            "ruby" => compare(
                &normalize(serde_json::to_value(ruby::drop_lines(&input)).unwrap()),
                &call["output"],
                &path,
            ),
            "size_ratio" => {
                let (ratio, basis, a, b) =
                    next_line::text_size_ratio_with_basis(&input[0], &input[1]);
                compare(&json!(ratio), &call["ratio"], &path);
                compare(&json!(basis), &call["basis"], &path);
                compare(&json!(a), &call["previousCompared"], &path);
                compare(&json!(b), &call["currentCompared"], &path);
            }
            _ => panic!("unknown recording kind {kind}"),
        }
    }
}

#[test]
fn a_label_standing_clear_above_a_line_is_kept_0() {
    check(include_str!(
        "fixtures/screen/grouping/a_label_standing_clear_above_a_line_is_kept_0.json"
    ));
}

#[test]
fn a_line_of_kana_the_same_size_as_the_one_below_is_kept_0() {
    check(include_str!(
        "fixtures/screen/grouping/a_line_of_kana_the_same_size_as_the_one_below_is_kept_0.json"
    ));
}

#[test]
fn a_one_character_reading_is_dropped_0() {
    check(include_str!(
        "fixtures/screen/grouping/a_one_character_reading_is_dropped_0.json"
    ));
}

#[test]
fn a_page_with_no_readings_is_returned_unchanged_0() {
    check(include_str!(
        "fixtures/screen/grouping/a_page_with_no_readings_is_returned_unchanged_0.json"
    ));
}

#[test]
fn a_reading_above_its_line_is_dropped_0() {
    check(include_str!(
        "fixtures/screen/grouping/a_reading_above_its_line_is_dropped_0.json"
    ));
}

#[test]
fn a_reading_over_a_line_it_does_not_overlap_is_kept_0() {
    check(include_str!(
        "fixtures/screen/grouping/a_reading_over_a_line_it_does_not_overlap_is_kept_0.json"
    ));
}

#[test]
fn a_sentence_under_its_readings_reaches_the_translator_alone_0() {
    check(include_str!("fixtures/screen/grouping/a_sentence_under_its_readings_reaches_the_translator_alone_0.json"));
}

#[test]
fn aballoon_interior_with_the_wide_line_first_is_still_read_0() {
    check(include_str!(
        "fixtures/screen/grouping/aballoon_interior_with_the_wide_line_first_is_still_read_0.json"
    ));
}

#[test]
fn aflush_left_stack_is_refused_even_in_the_general_mode_0() {
    check(include_str!(
        "fixtures/screen/grouping/aflush_left_stack_is_refused_even_in_the_general_mode_0.json"
    ));
}

#[test]
fn agroup_carries_the_median_font_height_of_its_lines_0() {
    check(include_str!(
        "fixtures/screen/grouping/agroup_carries_the_median_font_height_of_its_lines_0.json"
    ));
}

#[test]
fn alabel_ending_in_acolon_is_held_to_the_unrelaxed_leading_in_both_modes_0() {
    check(include_str!("fixtures/screen/grouping/alabel_ending_in_acolon_is_held_to_the_unrelaxed_leading_in_both_modes_0.json"));
}

#[test]
fn alabel_ending_in_acolon_still_joins_when_the_value_is_set_right_under_it_0() {
    check(include_str!("fixtures/screen/grouping/alabel_ending_in_acolon_still_joins_when_the_value_is_set_right_under_it_0.json"));
}

#[test]
fn alabel_over_abody_of_text_is_still_refused_by_the_alignment_gate_0() {
    check(include_str!("fixtures/screen/grouping/alabel_over_abody_of_text_is_still_refused_by_the_alignment_gate_0.json"));
}

#[test]
fn aline_beside_the_column_does_not_stop_two_lines_being_continuations_0() {
    check(include_str!("fixtures/screen/grouping/aline_beside_the_column_does_not_stop_two_lines_being_continuations_0.json"));
}

#[test]
fn aline_ending_on_acolon_continues_only_when_the_next_line_is_set_close_under_it_0() {
    check(include_str!("fixtures/screen/grouping/aline_ending_on_acolon_continues_only_when_the_next_line_is_set_close_under_it_0.json"));
}

#[test]
fn aline_of_nothing_but_figures_is_not_acontinuation_0() {
    check(include_str!(
        "fixtures/screen/grouping/aline_of_nothing_but_figures_is_not_acontinuation_0.json"
    ));
}

#[test]
fn aline_opening_with_abullet_is_never_acontinuation_0() {
    check(include_str!(
        "fixtures/screen/grouping/aline_opening_with_abullet_is_never_acontinuation_0.json"
    ));
}

#[test]
fn aline_standing_between_two_others_stops_them_being_continuations_0() {
    check(include_str!("fixtures/screen/grouping/aline_standing_between_two_others_stops_them_being_continuations_0.json"));
}

#[test]
fn aline_with_no_scores_carries_no_confidence_0() {
    check(include_str!(
        "fixtures/screen/grouping/aline_with_no_scores_carries_no_confidence_0.json"
    ));
}

#[test]
fn amenu_entry_stack_does_not_join_at_the_leading_aparagraph_is_allowed_0() {
    check(include_str!("fixtures/screen/grouping/amenu_entry_stack_does_not_join_at_the_leading_aparagraph_is_allowed_0.json"));
}

#[test]
fn amerge_that_ends_up_mixed_carries_no_estimate_at_all_0() {
    check(include_str!(
        "fixtures/screen/grouping/amerge_that_ends_up_mixed_carries_no_estimate_at_all_0.json"
    ));
}

#[test]
fn amerged_line_can_be_followed_back_to_the_boxes_it_was_built_from_0() {
    check(include_str!("fixtures/screen/grouping/amerged_line_can_be_followed_back_to_the_boxes_it_was_built_from_0.json"));
}

#[test]
fn amerged_line_is_as_confident_as_its_text_deserves_0() {
    check(include_str!(
        "fixtures/screen/grouping/amerged_line_is_as_confident_as_its_text_deserves_0.json"
    ));
}

#[test]
fn anavigation_column_beside_the_body_does_not_set_the_bodys_leading_0() {
    check(include_str!("fixtures/screen/grouping/anavigation_column_beside_the_body_does_not_set_the_bodys_leading_0.json"));
}

#[test]
fn apage_with_too_few_steps_has_no_leading_to_read_0() {
    check(include_str!(
        "fixtures/screen/grouping/apage_with_too_few_steps_has_no_leading_to_read_0.json"
    ));
}

#[test]
fn aparagraph_seam_joins_in_the_general_mode_and_stays_apart_in_the_interface_mode_0() {
    check(include_str!("fixtures/screen/grouping/aparagraph_seam_joins_in_the_general_mode_and_stays_apart_in_the_interface_mode_0.json"));
}

#[test]
fn aparagraph_seam_past_one_line_joins_when_the_line_above_could_have_wrapped_0() {
    check(include_str!("fixtures/screen/grouping/aparagraph_seam_past_one_line_joins_when_the_line_above_could_have_wrapped_0.json"));
}

#[test]
fn asentence_continuing_on_afigure_is_still_acontinuation_0() {
    check(include_str!(
        "fixtures/screen/grouping/asentence_continuing_on_afigure_is_still_acontinuation_0.json"
    ));
}

#[test]
fn aseparator_boxed_on_its_own_does_not_cut_the_row_0() {
    check(include_str!(
        "fixtures/screen/grouping/aseparator_boxed_on_its_own_does_not_cut_the_row_0.json"
    ));
}

#[test]
fn aseparator_boxed_on_its_own_does_not_cut_the_row_1() {
    check(include_str!(
        "fixtures/screen/grouping/aseparator_boxed_on_its_own_does_not_cut_the_row_1.json"
    ));
}

#[test]
fn aseparator_boxed_on_its_own_does_not_cut_the_row_2() {
    check(include_str!(
        "fixtures/screen/grouping/aseparator_boxed_on_its_own_does_not_cut_the_row_2.json"
    ));
}

#[test]
fn asettings_panels_entries_stay_apart_in_the_general_mode_too_by_two_hundredths_of_aline_0() {
    check(include_str!("fixtures/screen/grouping/asettings_panels_entries_stay_apart_in_the_general_mode_too_by_two_hundredths_of_aline_0.json"));
}

#[test]
fn ashort_bubble_opening_joins_in_comic_mode_and_not_in_standard_0() {
    check(include_str!("fixtures/screen/grouping/ashort_bubble_opening_joins_in_comic_mode_and_not_in_standard_0.json"));
}

#[test]
fn ashort_centred_line_above_is_still_judged_on_the_unrelaxed_limit_0() {
    check(include_str!("fixtures/screen/grouping/ashort_centred_line_above_is_still_judged_on_the_unrelaxed_limit_0.json"));
}

#[test]
fn ashort_line_between_two_slightly_sloped_long_ones_still_lies_between_them_0() {
    check(include_str!("fixtures/screen/grouping/ashort_line_between_two_slightly_sloped_long_ones_still_lies_between_them_0.json"));
}

#[test]
fn ashort_line_set_solid_is_not_joined_under_the_standard_profile_0() {
    check(include_str!("fixtures/screen/grouping/ashort_line_set_solid_is_not_joined_under_the_standard_profile_0.json"));
}

#[test]
fn ashort_smaller_cjk_suffix_does_not_cut_the_row_0() {
    check(include_str!(
        "fixtures/screen/grouping/ashort_smaller_cjk_suffix_does_not_cut_the_row_0.json"
    ));
}

#[test]
fn ashorter_line_set_too_far_below_is_not_the_end_of_the_paragraph_0() {
    check(include_str!("fixtures/screen/grouping/ashorter_line_set_too_far_below_is_not_the_end_of_the_paragraph_0.json"));
}

#[test]
fn asking_for_the_trace_changes_nothing_about_the_grouping_0() {
    check(include_str!(
        "fixtures/screen/grouping/asking_for_the_trace_changes_nothing_about_the_grouping_0.json"
    ));
}

#[test]
fn astar_before_aword_does_not_cut_the_row_0() {
    check(include_str!(
        "fixtures/screen/grouping/astar_before_aword_does_not_cut_the_row_0.json"
    ));
}

#[test]
fn auto_english_japanese_chinese_produce_same_grouping_for_ajapanese_paragraph_0() {
    check(include_str!("fixtures/screen/grouping/auto_english_japanese_chinese_produce_same_grouping_for_ajapanese_paragraph_0.json"));
}

#[test]
fn auto_english_japanese_chinese_produce_same_grouping_for_ajapanese_paragraph_1() {
    check(include_str!("fixtures/screen/grouping/auto_english_japanese_chinese_produce_same_grouping_for_ajapanese_paragraph_1.json"));
}

#[test]
fn auto_english_japanese_chinese_produce_same_grouping_for_amixed_capture_0() {
    check(include_str!("fixtures/screen/grouping/auto_english_japanese_chinese_produce_same_grouping_for_amixed_capture_0.json"));
}

#[test]
fn auto_english_japanese_chinese_produce_same_grouping_for_amixed_capture_1() {
    check(include_str!("fixtures/screen/grouping/auto_english_japanese_chinese_produce_same_grouping_for_amixed_capture_1.json"));
}

#[test]
fn aword_with_no_descender_is_still_part_of_its_line_0() {
    check(include_str!(
        "fixtures/screen/grouping/aword_with_no_descender_is_still_part_of_its_line_0.json"
    ));
}

#[test]
fn awrapped_title_joins_across_aline_from_another_column_0() {
    check(include_str!(
        "fixtures/screen/grouping/awrapped_title_joins_across_aline_from_another_column_0.json"
    ));
}

#[test]
fn bare_speaker_label_keeps_its_message_on_its_own_rows_0() {
    check(include_str!(
        "fixtures/screen/grouping/bare_speaker_label_keeps_its_message_on_its_own_rows_0.json"
    ));
}

#[test]
fn bigger_or_independent_warning_after_established_paragraph_stays_separate_0() {
    check(include_str!("fixtures/screen/grouping/bigger_or_independent_warning_after_established_paragraph_stays_separate_0.json"));
}

#[test]
fn box_heights_alone_never_break_arow_so_the_live_overlay_does_not_flicker_0() {
    check(include_str!("fixtures/screen/grouping/box_heights_alone_never_break_arow_so_the_live_overlay_does_not_flicker_0.json"));
}

#[test]
fn box_heights_alone_never_break_arow_so_the_live_overlay_does_not_flicker_1() {
    check(include_str!("fixtures/screen/grouping/box_heights_alone_never_break_arow_so_the_live_overlay_does_not_flicker_1.json"));
}

#[test]
fn captured_article_retains_every_fragment_in_reading_order_0() {
    check(include_str!(
        "fixtures/screen/grouping/captured_article_retains_every_fragment_in_reading_order_0.json"
    ));
}

#[test]
fn captured_article_retains_every_fragment_in_reading_order_1() {
    check(include_str!(
        "fixtures/screen/grouping/captured_article_retains_every_fragment_in_reading_order_1.json"
    ));
}

#[test]
fn captured_article_retains_every_fragment_in_reading_order_2() {
    check(include_str!(
        "fixtures/screen/grouping/captured_article_retains_every_fragment_in_reading_order_2.json"
    ));
}

#[test]
fn captured_article_retains_every_fragment_in_reading_order_3() {
    check(include_str!(
        "fixtures/screen/grouping/captured_article_retains_every_fragment_in_reading_order_3.json"
    ));
}

#[test]
fn centred_lines_are_no_longer_refused_by_the_alignment_gate_0() {
    check(include_str!(
        "fixtures/screen/grouping/centred_lines_are_no_longer_refused_by_the_alignment_gate_0.json"
    ));
}

#[test]
fn centred_speech_still_sets_solid_on_its_centre_0() {
    check(include_str!(
        "fixtures/screen/grouping/centred_speech_still_sets_solid_on_its_centre_0.json"
    ));
}

#[test]
fn chat_entry_set_solid_under_the_one_above_stays_apart_in_ascreenshot_too_0() {
    check(include_str!("fixtures/screen/grouping/chat_entry_set_solid_under_the_one_above_stays_apart_in_ascreenshot_too_0.json"));
}

#[test]
fn chat_entry_set_solid_under_the_one_above_stays_apart_in_ascreenshot_too_1() {
    check(include_str!("fixtures/screen/grouping/chat_entry_set_solid_under_the_one_above_stays_apart_in_ascreenshot_too_1.json"));
}

#[test]
fn chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_0() {
    check(include_str!("fixtures/screen/grouping/chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_0.json"));
}

#[test]
fn chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_1() {
    check(include_str!("fixtures/screen/grouping/chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_1.json"));
}

#[test]
fn chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_2() {
    check(include_str!("fixtures/screen/grouping/chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_2.json"));
}

#[test]
fn chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_3() {
    check(include_str!("fixtures/screen/grouping/chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_3.json"));
}

#[test]
fn chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_4() {
    check(include_str!("fixtures/screen/grouping/chat_entry_set_solid_under_the_one_above_stays_apart_on_the_live_panel_path_4.json"));
}

#[test]
fn chinese_body_text_joins_at_its_pages_own_leading_0() {
    check(include_str!(
        "fixtures/screen/grouping/chinese_body_text_joins_at_its_pages_own_leading_0.json"
    ));
}

#[test]
fn comic_mode_does_not_waive_the_set_solid_geometry_0() {
    check(include_str!(
        "fixtures/screen/grouping/comic_mode_does_not_waive_the_set_solid_geometry_0.json"
    ));
}

#[test]
fn cross_script_pair_with_equal_detection_boxes_is_not_refused_by_size_test_0() {
    check(include_str!("fixtures/screen/grouping/cross_script_pair_with_equal_detection_boxes_is_not_refused_by_size_test_0.json"));
}

#[test]
fn earlier_mixed_size_and_direction_counterexamples_do_not_regress_0() {
    check(include_str!("fixtures/screen/grouping/earlier_mixed_size_and_direction_counterexamples_do_not_regress_0.json"));
}

#[test]
fn empty_corner_of_aparagraph_does_not_absorb_an_independent_label_0() {
    check(include_str!("fixtures/screen/grouping/empty_corner_of_aparagraph_does_not_absorb_an_independent_label_0.json"));
}

#[test]
fn flush_right_lines_are_no_longer_refused_by_the_alignment_gate_0() {
    check(include_str!("fixtures/screen/grouping/flush_right_lines_are_no_longer_refused_by_the_alignment_gate_0.json"));
}

#[test]
fn general_matches_marked_paragraph_members_0() {
    check(include_str!(
        "fixtures/screen/grouping/general_matches_marked_paragraph_members_0.json"
    ));
}

#[test]
fn general_matches_marked_paragraph_members_1() {
    check(include_str!(
        "fixtures/screen/grouping/general_matches_marked_paragraph_members_1.json"
    ));
}

#[test]
fn general_matches_marked_paragraph_members_2() {
    check(include_str!(
        "fixtures/screen/grouping/general_matches_marked_paragraph_members_2.json"
    ));
}

#[test]
fn general_matches_marked_paragraph_members_3() {
    check(include_str!(
        "fixtures/screen/grouping/general_matches_marked_paragraph_members_3.json"
    ));
}

#[test]
fn general_matches_marked_paragraph_members_4() {
    check(include_str!(
        "fixtures/screen/grouping/general_matches_marked_paragraph_members_4.json"
    ));
}

#[test]
fn groups_adjacent_aligned_lines_into_single_block_0() {
    check(include_str!(
        "fixtures/screen/grouping/groups_adjacent_aligned_lines_into_single_block_0.json"
    ));
}

#[test]
fn groups_awrapped_line_whose_box_is_short_for_want_of_an_ascender_0() {
    check(include_str!("fixtures/screen/grouping/groups_awrapped_line_whose_box_is_short_for_want_of_an_ascender_0.json"));
}

#[test]
fn groups_awrapped_line_whose_boxes_overlap_by_the_detectors_expansion_0() {
    check(include_str!("fixtures/screen/grouping/groups_awrapped_line_whose_boxes_overlap_by_the_detectors_expansion_0.json"));
}

#[test]
fn groups_wrapped_sentence_when_opening_quote_continues_on_next_line_0() {
    check(include_str!("fixtures/screen/grouping/groups_wrapped_sentence_when_opening_quote_continues_on_next_line_0.json"));
}

#[test]
fn headlines_further_apart_than_the_cap_stay_apart_on_aloosely_set_page_0() {
    check(include_str!("fixtures/screen/grouping/headlines_further_apart_than_the_cap_stay_apart_on_aloosely_set_page_0.json"));
}

#[test]
fn independent_columns_can_still_continue_downward_0() {
    check(include_str!(
        "fixtures/screen/grouping/independent_columns_can_still_continue_downward_0.json"
    ));
}

#[test]
fn interface_keeps_its_existing_members_for_all_five_images_0() {
    check(include_str!(
        "fixtures/screen/grouping/interface_keeps_its_existing_members_for_all_five_images_0.json"
    ));
}

#[test]
fn isolated_short_final_word_does_not_get_the_paragraph_exception_0() {
    check(include_str!("fixtures/screen/grouping/isolated_short_final_word_does_not_get_the_paragraph_exception_0.json"));
}

#[test]
fn kana_above_a_line_without_kanji_is_kept_0() {
    check(include_str!(
        "fixtures/screen/grouping/kana_above_a_line_without_kanji_is_kept_0.json"
    ));
}

#[test]
fn katakana_above_a_line_is_kept_0() {
    check(include_str!(
        "fixtures/screen/grouping/katakana_above_a_line_is_kept_0.json"
    ));
}

#[test]
fn keeps_aligned_independent_lines_separate_when_widths_are_similar_0() {
    check(include_str!("fixtures/screen/grouping/keeps_aligned_independent_lines_separate_when_widths_are_similar_0.json"));
}

#[test]
fn keeps_boxes_that_share_arow_out_of_the_next_line_join_0() {
    check(include_str!(
        "fixtures/screen/grouping/keeps_boxes_that_share_arow_out_of_the_next_line_join_0.json"
    ));
}

#[test]
fn keeps_different_columns_separate_0() {
    check(include_str!(
        "fixtures/screen/grouping/keeps_different_columns_separate_0.json"
    ));
}

#[test]
fn keeps_different_font_sizes_separate_0() {
    check(include_str!(
        "fixtures/screen/grouping/keeps_different_font_sizes_separate_0.json"
    ));
}

#[test]
fn keeps_lines_of_different_text_sizes_apart_even_when_their_boxes_match_0() {
    check(include_str!("fixtures/screen/grouping/keeps_lines_of_different_text_sizes_apart_even_when_their_boxes_match_0.json"));
}

#[test]
fn keeps_next_sentence_separate_after_sentence_terminator_0() {
    check(include_str!(
        "fixtures/screen/grouping/keeps_next_sentence_separate_after_sentence_terminator_0.json"
    ));
}

#[test]
fn keeps_separate_phrases_on_the_same_line_when_gap_is_large_0() {
    check(include_str!(
        "fixtures/screen/grouping/keeps_separate_phrases_on_the_same_line_when_gap_is_large_0.json"
    ));
}

#[test]
fn keeps_stacked_menu_labels_separate_0() {
    check(include_str!(
        "fixtures/screen/grouping/keeps_stacked_menu_labels_separate_0.json"
    ));
}

#[test]
fn keeps_title_and_body_separate_when_heights_are_close_but_distinct_0() {
    check(include_str!("fixtures/screen/grouping/keeps_title_and_body_separate_when_heights_are_close_but_distinct_0.json"));
}

#[test]
fn keeps_two_distinct_same_row_buttons_separate_0() {
    check(include_str!(
        "fixtures/screen/grouping/keeps_two_distinct_same_row_buttons_separate_0.json"
    ));
}

#[test]
fn lines_are_numbered_in_the_order_the_next_line_rules_see_them_0() {
    check(include_str!("fixtures/screen/grouping/lines_are_numbered_in_the_order_the_next_line_rules_see_them_0.json"));
}

#[test]
fn lines_at_one_leading_but_out_of_line_are_not_set_solid_0() {
    check(include_str!(
        "fixtures/screen/grouping/lines_at_one_leading_but_out_of_line_are_not_set_solid_0.json"
    ));
}

#[test]
fn list_entries_just_past_the_relaxed_limit_stay_apart_0() {
    check(include_str!(
        "fixtures/screen/grouping/list_entries_just_past_the_relaxed_limit_stay_apart_0.json"
    ));
}

#[test]
fn list_entries_just_past_the_relaxed_limit_stay_apart_1() {
    check(include_str!(
        "fixtures/screen/grouping/list_entries_just_past_the_relaxed_limit_stay_apart_1.json"
    ));
}

#[test]
fn list_entries_just_past_the_relaxed_limit_stay_apart_2() {
    check(include_str!(
        "fixtures/screen/grouping/list_entries_just_past_the_relaxed_limit_stay_apart_2.json"
    ));
}

#[test]
fn list_entries_just_past_the_relaxed_limit_stay_apart_3() {
    check(include_str!(
        "fixtures/screen/grouping/list_entries_just_past_the_relaxed_limit_stay_apart_3.json"
    ));
}

#[test]
fn list_entries_set_looser_than_the_solid_limit_are_not_set_solid_0() {
    check(include_str!("fixtures/screen/grouping/list_entries_set_looser_than_the_solid_limit_are_not_set_solid_0.json"));
}

#[test]
fn list_lines_whose_centres_meet_by_chance_are_not_set_solid_0() {
    check(include_str!(
        "fixtures/screen/grouping/list_lines_whose_centres_meet_by_chance_are_not_set_solid_0.json"
    ));
}

#[test]
fn merges_adjacent_fragments_on_the_same_line_0() {
    check(include_str!(
        "fixtures/screen/grouping/merges_adjacent_fragments_on_the_same_line_0.json"
    ));
}

#[test]
fn merges_heading_word_boxes_that_overlap_horizontally_from_unclip_expansion_0() {
    check(include_str!("fixtures/screen/grouping/merges_heading_word_boxes_that_overlap_horizontally_from_unclip_expansion_0.json"));
}

#[test]
fn merges_per_word_boxes_on_one_line_when_box_tops_vary_from_ascenders_and_descenders_0() {
    check(include_str!("fixtures/screen/grouping/merges_per_word_boxes_on_one_line_when_box_tops_vary_from_ascenders_and_descenders_0.json"));
}

#[test]
fn merges_short_mid_line_word_that_is_fully_nested_in_the_line_0() {
    check(include_str!("fixtures/screen/grouping/merges_short_mid_line_word_that_is_fully_nested_in_the_line_0.json"));
}

#[test]
fn mixed_english_japanese_does_not_cross_merge_paragraphs_0() {
    check(include_str!(
        "fixtures/screen/grouping/mixed_english_japanese_does_not_cross_merge_paragraphs_0.json"
    ));
}

#[test]
fn mixed_script_block_does_not_use_cjk_glyph_metric_for_latin_dominant_text_0() {
    check(include_str!("fixtures/screen/grouping/mixed_script_block_does_not_use_cjk_glyph_metric_for_latin_dominant_text_0.json"));
}

#[test]
fn mixed_script_block_uses_layout_bounds_for_size_comparison_0() {
    check(include_str!(
        "fixtures/screen/grouping/mixed_script_block_uses_layout_bounds_for_size_comparison_0.json"
    ));
}

#[test]
fn next_line_diagnostic_reports_centre_and_right_deltas_not_only_the_left_edge_0() {
    check(include_str!("fixtures/screen/grouping/next_line_diagnostic_reports_centre_and_right_deltas_not_only_the_left_edge_0.json"));
}

#[test]
fn ordinary_lines_do_not_open_like_aspeaker_0() {
    check(include_str!(
        "fixtures/screen/grouping/ordinary_lines_do_not_open_like_aspeaker_0.json"
    ));
}

#[test]
fn ordinary_lines_do_not_open_like_aspeaker_1() {
    check(include_str!(
        "fixtures/screen/grouping/ordinary_lines_do_not_open_like_aspeaker_1.json"
    ));
}

#[test]
fn ordinary_lines_do_not_open_like_aspeaker_2() {
    check(include_str!(
        "fixtures/screen/grouping/ordinary_lines_do_not_open_like_aspeaker_2.json"
    ));
}

#[test]
fn paragraph_middle_lines_of_equal_width_join_on_the_setting_alone_0() {
    check(include_str!("fixtures/screen/grouping/paragraph_middle_lines_of_equal_width_join_on_the_setting_alone_0.json"));
}

#[test]
fn pieces_of_one_cell_still_join_at_any_size_0() {
    check(include_str!(
        "fixtures/screen/grouping/pieces_of_one_cell_still_join_at_any_size_0.json"
    ));
}

#[test]
fn same_line_diagnostic_leaves_the_vertical_only_fields_at_zero_0() {
    check(include_str!("fixtures/screen/grouping/same_line_diagnostic_leaves_the_vertical_only_fields_at_zero_0.json"));
}

#[test]
fn same_line_gap_is_measured_on_layout_bounds_not_the_normalised_box_0() {
    check(include_str!("fixtures/screen/grouping/same_line_gap_is_measured_on_layout_bounds_not_the_normalised_box_0.json"));
}

#[test]
fn same_line_gaps_are_judged_against_the_spacing_of_this_capture_0() {
    check(include_str!("fixtures/screen/grouping/same_line_gaps_are_judged_against_the_spacing_of_this_capture_0.json"));
}

#[test]
fn separate_columns_without_aspanning_continuation_stay_separate_0() {
    check(include_str!("fixtures/screen/grouping/separate_columns_without_aspanning_continuation_stay_separate_0.json"));
}

#[test]
fn short_entries_at_that_leading_stay_alist_0() {
    check(include_str!(
        "fixtures/screen/grouping/short_entries_at_that_leading_stay_alist_0.json"
    ));
}

#[test]
fn small_centred_lines_padded_unevenly_still_set_solid_0() {
    check(include_str!(
        "fixtures/screen/grouping/small_centred_lines_padded_unevenly_still_set_solid_0.json"
    ));
}

#[test]
fn small_centred_lines_padded_unevenly_still_set_solid_1() {
    check(include_str!(
        "fixtures/screen/grouping/small_centred_lines_padded_unevenly_still_set_solid_1.json"
    ));
}

#[test]
fn small_centred_lines_padded_unevenly_still_set_solid_2() {
    check(include_str!(
        "fixtures/screen/grouping/small_centred_lines_padded_unevenly_still_set_solid_2.json"
    ));
}

#[test]
fn small_centred_lines_padded_unevenly_still_set_solid_3() {
    check(include_str!(
        "fixtures/screen/grouping/small_centred_lines_padded_unevenly_still_set_solid_3.json"
    ));
}

#[test]
fn spanning_previous_line_cannot_take_only_one_half_of_the_following_row_0() {
    check(include_str!("fixtures/screen/grouping/spanning_previous_line_cannot_take_only_one_half_of_the_following_row_0.json"));
}

#[test]
fn split_visual_row_cannot_be_skipped_by_awrapping_line_0() {
    check(include_str!(
        "fixtures/screen/grouping/split_visual_row_cannot_be_skipped_by_awrapping_line_0.json"
    ));
}

#[test]
fn split_visual_row_cannot_be_skipped_by_awrapping_line_1() {
    check(include_str!(
        "fixtures/screen/grouping/split_visual_row_cannot_be_skipped_by_awrapping_line_1.json"
    ));
}

#[test]
fn split_visual_row_cannot_be_skipped_by_awrapping_line_2() {
    check(include_str!(
        "fixtures/screen/grouping/split_visual_row_cannot_be_skipped_by_awrapping_line_2.json"
    ));
}

#[test]
fn split_visual_row_cannot_be_skipped_by_awrapping_line_3() {
    check(include_str!(
        "fixtures/screen/grouping/split_visual_row_cannot_be_skipped_by_awrapping_line_3.json"
    ));
}

#[test]
fn stacked_controls_of_similar_height_are_still_kept_apart_0() {
    check(include_str!(
        "fixtures/screen/grouping/stacked_controls_of_similar_height_are_still_kept_apart_0.json"
    ));
}

#[test]
fn still_groups_asentence_long_enough_to_have_wrapped_0() {
    check(include_str!(
        "fixtures/screen/grouping/still_groups_asentence_long_enough_to_have_wrapped_0.json"
    ));
}

#[test]
fn synthetic_flat_background_ink_separates_heading_without_changing_render_metrics_0() {
    check(include_str!("fixtures/screen/grouping/synthetic_flat_background_ink_separates_heading_without_changing_render_metrics_0.json"));
}

#[test]
fn the_pages_leading_is_read_by_the_screenshot_mode_only_0() {
    check(include_str!(
        "fixtures/screen/grouping/the_pages_leading_is_read_by_the_screenshot_mode_only_0.json"
    ));
}

#[test]
fn the_readings_room_is_given_to_the_drawn_box_and_not_to_the_measured_one_0() {
    check(include_str!("fixtures/screen/grouping/the_readings_room_is_given_to_the_drawn_box_and_not_to_the_measured_one_0.json"));
}

#[test]
fn the_relaxed_size_ratio_does_not_reach_pairs_that_are_not_set_solid_0() {
    check(include_str!("fixtures/screen/grouping/the_relaxed_size_ratio_does_not_reach_pairs_that_are_not_set_solid_0.json"));
}

#[test]
fn the_trace_reports_the_solid_limit_the_pair_was_judged_on_0() {
    check(include_str!(
        "fixtures/screen/grouping/the_trace_reports_the_solid_limit_the_pair_was_judged_on_0.json"
    ));
}

#[test]
fn three_fragments_combine_one_merge_at_atime_and_not_as_amedian_of_the_three_0() {
    check(include_str!("fixtures/screen/grouping/three_fragments_combine_one_merge_at_atime_and_not_as_amedian_of_the_three_0.json"));
}

#[test]
fn title_beside_asmaller_explanation_is_not_one_line_0() {
    check(include_str!(
        "fixtures/screen/grouping/title_beside_asmaller_explanation_is_not_one_line_0.json"
    ));
}

#[test]
fn two_balloon_lines_of_equal_width_are_read_on_their_centres_0() {
    check(include_str!("fixtures/screen/grouping/two_balloon_lines_of_equal_width_are_read_on_their_centres_0.json"));
}

#[test]
fn two_short_readouts_on_one_centre_are_refused_on_their_length_0() {
    check(include_str!("fixtures/screen/grouping/two_short_readouts_on_one_centre_are_refused_on_their_length_0.json"));
}

#[test]
fn uneven_hand_lettering_joins_in_the_general_mode_on_the_set_solid_path_0() {
    check(include_str!("fixtures/screen/grouping/uneven_hand_lettering_joins_in_the_general_mode_on_the_set_solid_path_0.json"));
}

#[test]
fn wrapped_final_line_leading_is_measured_on_layout_bounds_not_the_normalised_box_0() {
    check(include_str!("fixtures/screen/grouping/wrapped_final_line_leading_is_measured_on_layout_bounds_not_the_normalised_box_0.json"));
}

#[test]
fn wrapped_prose_rejoins_fragments_across_an_unrecognised_dash_0() {
    check(include_str!("fixtures/screen/grouping/wrapped_prose_rejoins_fragments_across_an_unrecognised_dash_0.json"));
}

#[test]
fn wrapped_prose_rejoins_fragments_across_an_unrecognised_dash_1() {
    check(include_str!("fixtures/screen/grouping/wrapped_prose_rejoins_fragments_across_an_unrecognised_dash_1.json"));
}

#[test]
fn wrapped_prose_rejoins_fragments_across_an_unrecognised_dash_2() {
    check(include_str!("fixtures/screen/grouping/wrapped_prose_rejoins_fragments_across_an_unrecognised_dash_2.json"));
}

#[test]
fn wrapped_sentence_still_joins_in_ascreenshot_0() {
    check(include_str!(
        "fixtures/screen/grouping/wrapped_sentence_still_joins_in_ascreenshot_0.json"
    ));
}

#[test]
fn wrapped_sentence_still_joins_on_the_live_panel_path_0() {
    check(include_str!(
        "fixtures/screen/grouping/wrapped_sentence_still_joins_on_the_live_panel_path_0.json"
    ));
}

#[test]
fn independent_rows_stay_separate_0() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_0.json"
    ));
}

#[test]
fn independent_rows_stay_separate_1() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_1.json"
    ));
}

#[test]
fn independent_rows_stay_separate_2() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_2.json"
    ));
}

#[test]
fn independent_rows_stay_separate_3() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_3.json"
    ));
}

#[test]
fn independent_rows_stay_separate_4() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_4.json"
    ));
}

#[test]
fn independent_rows_stay_separate_5() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_5.json"
    ));
}

#[test]
fn independent_rows_stay_separate_6() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_6.json"
    ));
}

#[test]
fn independent_rows_stay_separate_7() {
    check(include_str!(
        "fixtures/screen/grouping/independent_rows_stay_separate_7.json"
    ));
}

#[test]
fn insufficient_or_column_like_evidence_does_not_enable_new_row_merges_0() {
    check(include_str!("fixtures/screen/grouping/insufficient_or_column_like_evidence_does_not_enable_new_row_merges_0.json"));
}

#[test]
fn insufficient_or_column_like_evidence_does_not_enable_new_row_merges_1() {
    check(include_str!("fixtures/screen/grouping/insufficient_or_column_like_evidence_does_not_enable_new_row_merges_1.json"));
}

#[test]
fn insufficient_or_column_like_evidence_does_not_enable_new_row_merges_2() {
    check(include_str!("fixtures/screen/grouping/insufficient_or_column_like_evidence_does_not_enable_new_row_merges_2.json"));
}

#[test]
fn insufficient_or_column_like_evidence_does_not_enable_new_row_merges_3() {
    check(include_str!("fixtures/screen/grouping/insufficient_or_column_like_evidence_does_not_enable_new_row_merges_3.json"));
}

#[test]
fn insufficient_or_column_like_evidence_does_not_enable_new_row_merges_4() {
    check(include_str!("fixtures/screen/grouping/insufficient_or_column_like_evidence_does_not_enable_new_row_merges_4.json"));
}

#[test]
fn insufficient_or_column_like_evidence_does_not_enable_new_row_merges_5() {
    check(include_str!("fixtures/screen/grouping/insufficient_or_column_like_evidence_does_not_enable_new_row_merges_5.json"));
}

#[test]
fn japanese_game_body_rows_join_without_speaker_or_controls_0() {
    check(include_str!(
        "fixtures/screen/grouping/japanese_game_body_rows_join_without_speaker_or_controls_0.json"
    ));
}

#[test]
fn non_general_profiles_keep_their_existing_row_decisions_0() {
    check(include_str!(
        "fixtures/screen/grouping/non_general_profiles_keep_their_existing_row_decisions_0.json"
    ));
}

#[test]
fn staggered_cuts_under_acomplete_row_rejoin_in_visual_order_0() {
    check(include_str!(
        "fixtures/screen/grouping/staggered_cuts_under_acomplete_row_rejoin_in_visual_order_0.json"
    ));
}

#[test]
fn staggered_cuts_under_acomplete_row_rejoin_in_visual_order_1() {
    check(include_str!(
        "fixtures/screen/grouping/staggered_cuts_under_acomplete_row_rejoin_in_visual_order_1.json"
    ));
}

#[test]
fn differing_scripts_fall_back_to_the_boxes_even_with_both_estimates_in_hand_0() {
    check(include_str!("fixtures/screen/grouping/differing_scripts_fall_back_to_the_boxes_even_with_both_estimates_in_hand_0.json"));
}

#[test]
fn one_script_with_amissing_estimate_is_the_other_reason_entirely_0() {
    check(include_str!("fixtures/screen/grouping/one_script_with_amissing_estimate_is_the_other_reason_entirely_0.json"));
}

#[test]
fn two_lines_of_one_script_are_compared_on_their_estimates_0() {
    check(include_str!(
        "fixtures/screen/grouping/two_lines_of_one_script_are_compared_on_their_estimates_0.json"
    ));
}
