use lt_core::screen::{
    erase::RemovalBlock,
    geom::RectF,
    imaging::{ImageBuffer, PixelType},
    paint::{
        backdrop::{self, Backdrop},
        color, sampling,
        vote::DominantColorVote,
    },
    ImageView,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap, fs::File, io::BufReader, path::PathBuf, sync::atomic::AtomicBool,
};

fn rgb(value: &Value) -> [u8; 3] {
    serde_json::from_value(value.clone()).unwrap()
}
fn rect(value: &Value) -> RectF {
    serde_json::from_value(value.clone()).unwrap()
}
fn lines(value: &Value) -> Option<Vec<RectF>> {
    serde_json::from_value(value.clone()).unwrap()
}
fn pair(value: Option<([u8; 3], [u8; 3])>) -> Value {
    value.map_or(
        Value::Null,
        |(background, text)| json!({"background":background,"text":text}),
    )
}
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/screen/paint")
}
fn image(name: &str) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(BufReader::new(
        File::open(fixture_dir().join(name)).unwrap(),
    ));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    let mut bgra = Vec::with_capacity((info.width * info.height * 4) as usize);
    match info.color_type {
        png::ColorType::Rgba => {
            for c in pixels[..info.buffer_size()].as_chunks::<4>().0 {
                bgra.extend_from_slice(&[c[2], c[1], c[0], c[3]]);
            }
        }
        png::ColorType::Rgb => {
            for c in pixels[..info.buffer_size()].as_chunks::<3>().0 {
                bgra.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        other => panic!("unexpected pixels: {other:?}"),
    }
    (info.width, info.height, bgra)
}
fn run(text: &str) {
    let test: Value = serde_json::from_str(text).unwrap();
    let mut backdrops = BTreeMap::new();
    for (index, call) in test["calls"].as_array().unwrap().iter().enumerate() {
        let input = &call["input"];
        let pixels = input["image"].as_str().map(image);
        let view = pixels
            .as_ref()
            .map(|(w, h, bgra)| ImageView::new(*w, *h, *w * 4, bgra).unwrap());
        let result = match call["op"].as_str().unwrap() {
            "sample" => {
                let result = sampling::sample(
                    view.unwrap(),
                    rect(&input["bounds"]),
                    input["backgroundOverride"]
                        .as_array()
                        .map(|_| rgb(&input["backgroundOverride"])),
                    input["ringReference"].as_f64(),
                );
                result.map_or(
                    Value::Null,
                    |r| json!({"surface":r.surface,"body":r.body,"outline":r.outline}),
                )
            }
            "capture" => pair(Some(sampling::for_capture_overlay(
                view.unwrap(),
                rect(&input["bounds"]),
                lines(&input["sourceLines"]).as_deref(),
                input["vertical"].as_bool().unwrap(),
            ))),
            "between" => json!(sampling::between_lines(
                view.unwrap(),
                &lines(&input["lines"]).unwrap(),
                input["vertical"].as_bool().unwrap()
            )),
            "vote" => {
                let mut vote = DominantColorVote::default();
                for c in input["pixels"].as_array().unwrap() {
                    vote.add(rgb(c));
                }
                json!(vote.dominant())
            }
            "tune" => json!(color::tune(rgb(&input["text"]), rgb(&input["background"]))),
            "contrast" => json!(color::ensure_contrast(
                rgb(&input["text"]),
                rgb(&input["background"]),
                input["minRatio"].as_f64().unwrap()
            )),
            "feather" => json!(backdrop::feather(input["height"].as_f64().unwrap())),
            "create" => {
                let blocks: Vec<_> = input["blocks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| RemovalBlock {
                        translated_text: b["translatedText"].as_str().unwrap().to_owned(),
                        bounds: rect(&b["bounds"]),
                        source_line_bounds: lines(&b["sourceLineBounds"]),
                        glyph_height: b["glyphHeight"].as_f64(),
                        within: serde_json::from_value(b["within"].clone()).unwrap(),
                    })
                    .collect();
                let backdrop =
                    Backdrop::create(view.unwrap(), &blocks, &AtomicBool::new(false)).unwrap();
                let exists = backdrop.is_some();
                if let Some(backdrop) = backdrop {
                    backdrops.insert(input["id"].as_u64().unwrap(), backdrop);
                }
                json!(exists)
            }
            "from_repaired" => {
                let (w, h, pixels) = pixels.as_ref().unwrap();
                let image = ImageBuffer::new(*w as usize, *h as usize, PixelType::U8C3);
                for y in 0..*h as usize {
                    for x in 0..*w as usize {
                        for c in 0..3 {
                            image.set_byte(y, x * 3 + c, pixels[(y * *w as usize + x) * 4 + c]);
                        }
                    }
                }
                let backdrop = Backdrop::from_repaired(&image);
                let exists = backdrop.is_some();
                if let Some(backdrop) = backdrop {
                    backdrops.insert(input["id"].as_u64().unwrap(), backdrop);
                }
                json!(exists)
            }
            "card" => pair(
                backdrops[&input["id"].as_u64().unwrap()]
                    .card(rect(&input["bubble"]), rgb(&input["text"])),
            ),
            "uniformity" => json!(backdrops[&input["id"].as_u64().unwrap()]
                .uniformity(rect(&input["area"]), input["glyphHeight"].as_f64().unwrap())),
            "plate" => {
                let result = backdrops[&input["id"].as_u64().unwrap()].plate(
                    rect(&input["area"]),
                    rgb(&input["wash"]),
                    rgb(&input["text"]),
                    input["glyphHeight"].as_f64().unwrap(),
                    input["writtenHeight"].as_f64().unwrap(),
                    None,
                );
                result.map_or(Value::Null,|r|json!({"width":r.width,"height":r.height,"text":r.text,"sha256":format!("{:x}",Sha256::digest(r.bgra))}))
            }
            op => panic!("unexpected operation {op}"),
        };
        if result.is_number() {
            assert_eq!(
                result.as_f64(),
                call["expected"].as_f64(),
                "{} call {index}: {}",
                test["test"],
                call["op"]
            );
        } else {
            assert_eq!(
                result, call["expected"],
                "{} call {index}: {}",
                test["test"], call["op"]
            );
        }
    }
}

macro_rules! case {
    ($name:ident, $file:literal) => {
        #[test]
        fn $name() {
            run(include_str!(concat!("fixtures/screen/paint/", $file)));
        }
    };
}
case!(between_lines_is_null_for_asingle_line_because_there_is_no_interior_to_read, "CaptureBackgroundColorTests.BetweenLines_IsNullForASingleLineBecauseThereIsNoInteriorToRead.json");
case!(
    between_lines_is_null_when_the_gaps_hold_no_single_colour,
    "CaptureBackgroundColorTests.BetweenLines_IsNullWhenTheGapsHoldNoSingleColour.json"
);
case!(between_lines_reads_the_gaps_beside_the_columns_when_the_text_is_vertical, "CaptureBackgroundColorTests.BetweenLines_ReadsTheGapsBesideTheColumnsWhenTheTextIsVertical.json");
case!(
    between_lines_reads_the_panel_the_paragraph_is_printed_on,
    "CaptureBackgroundColorTests.BetweenLines_ReadsThePanelTheParagraphIsPrintedOn.json"
);
case!(
    for_capture_overlay_leaves_asingle_line_exactly_as_it_was,
    "CaptureBackgroundColorTests.ForCaptureOverlay_LeavesASingleLineExactlyAsItWas.json"
);
case!(for_capture_overlay_takes_the_panel_rather_than_the_page_behind_it_for_aparagraph, "CaptureBackgroundColorTests.ForCaptureOverlay_TakesThePanelRatherThanThePageBehindItForAParagraph.json");
case!(
    card_answers_atext_colour_that_can_be_read_on_it,
    "CaptureBubbleBackdropTests.Card_AnswersATextColourThatCanBeReadOnIt.json"
);
case!(card_follows_what_the_bubble_covers_rather_than_what_the_source_line_sat_on, "CaptureBubbleBackdropTests.Card_FollowsWhatTheBubbleCoversRatherThanWhatTheSourceLineSatOn.json");
case!(
    create_is_null_with_no_blocks_to_repair,
    "CaptureBubbleBackdropTests.Create_IsNullWithNoBlocksToRepair.json"
);
case!(
    feather_stays_outside_the_bubble_even_for_tiny_text,
    "CaptureBubbleBackdropTests.Feather_StaysOutsideTheBubbleEvenForTinyText.json"
);
case!(
    from_repaired_answers_as_create_does_for_the_same_repair,
    "CaptureBubbleBackdropTests.FromRepaired_AnswersAsCreateDoesForTheSameRepair.json"
);
case!(
    plate_has_no_trace_of_the_source_text,
    "CaptureBubbleBackdropTests.Plate_HasNoTraceOfTheSourceText.json"
);
case!(
    plate_is_built_for_aphotographed_surface,
    "CaptureBubbleBackdropTests.Plate_IsBuiltForAPhotographedSurface.json"
);
case!(
    plate_is_null_for_an_area_outside_the_capture,
    "CaptureBubbleBackdropTests.Plate_IsNullForAnAreaOutsideTheCapture.json"
);
case!(
    plate_is_null_on_aflat_surface_so_the_caller_keeps_its_card,
    "CaptureBubbleBackdropTests.Plate_IsNullOnAFlatSurfaceSoTheCallerKeepsItsCard.json"
);
case!(plate_is_null_on_asharp_interface_even_where_the_bubble_covers_more_than_one_colour, "CaptureBubbleBackdropTests.Plate_IsNullOnASharpInterfaceEvenWhereTheBubbleCoversMoreThanOneColour.json");
case!(
    plate_is_opaque_in_the_middle_and_faded_at_the_edge,
    "CaptureBubbleBackdropTests.Plate_IsOpaqueInTheMiddleAndFadedAtTheEdge.json"
);
case!(
    plate_is_the_full_size_asked_for_even_where_the_capture_does_not_reach,
    "CaptureBubbleBackdropTests.Plate_IsTheFullSizeAskedForEvenWhereTheCaptureDoesNotReach.json"
);
case!(
    plate_keeps_the_gradient_it_was_cut_from_rather_than_flattening_it,
    "CaptureBubbleBackdropTests.Plate_KeepsTheGradientItWasCutFromRatherThanFlatteningIt.json"
);
case!(
    plate_keeps_white_text_white_by_darkening_what_it_crosses,
    "CaptureBubbleBackdropTests.Plate_KeepsWhiteTextWhiteByDarkeningWhatItCrosses.json"
);
case!(
    plate_moves_the_text_rather_than_the_picture_when_it_cannot_be_read,
    "CaptureBubbleBackdropTests.Plate_MovesTheTextRatherThanThePictureWhenItCannotBeRead.json"
);
case!(
    plate_takes_the_text_well_past_the_minimum_when_moving_it_is_free,
    "CaptureBubbleBackdropTests.Plate_TakesTheTextWellPastTheMinimumWhenMovingItIsFree.json"
);
case!(
    dominant_averages_nearby_shades_of_the_same_colour,
    "DominantColorVoteTests.Dominant_AveragesNearbyShadesOfTheSameColour.json"
);
case!(
    dominant_is_null_when_nothing_was_added,
    "DominantColorVoteTests.Dominant_IsNullWhenNothingWasAdded.json"
);
case!(
    dominant_keeps_the_majority_colour_instead_of_blending_in_the_minority,
    "DominantColorVoteTests.Dominant_KeepsTheMajorityColourInsteadOfBlendingInTheMinority.json"
);
case!(ensure_contrast_keeps_the_hue_of_saturated_text_on_adark_background_of_the_same_hue, "OverlayTextColorTests.EnsureContrast_KeepsTheHueOfSaturatedTextOnADarkBackgroundOfTheSameHue.json");
case!(
    ensure_contrast_leaves_areadable_pair_untouched,
    "OverlayTextColorTests.EnsureContrast_LeavesAReadablePairUntouched.json"
);
case!(
    ensure_contrast_rescues_white_poster_text_tuned_into_its_pale_card,
    "OverlayTextColorTests.EnsureContrast_RescuesWhitePosterTextTunedIntoItsPaleCard.json"
);
case!(for_capture_overlay_falls_back_to_black_on_white_when_the_box_is_outside_the_frame, "SourceTextColorSamplerTests.ForCaptureOverlay_FallsBackToBlackOnWhiteWhenTheBoxIsOutsideTheFrame.json");
case!(
    for_capture_overlay_keeps_white_caption_on_apale_card_legible,
    "SourceTextColorSamplerTests.ForCaptureOverlay_KeepsWhiteCaptionOnAPaleCardLegible.json"
);
case!(sample_has_no_text_colour_when_nothing_inside_stands_out_from_the_background, "SourceTextColorSamplerTests.Sample_HasNoTextColourWhenNothingInsideStandsOutFromTheBackground.json");
case!(
    sample_is_null_for_abox_outside_the_frame,
    "SourceTextColorSamplerTests.Sample_IsNullForABoxOutsideTheFrame.json"
);
case!(sample_reads_the_ring_around_the_box_as_background_and_the_majority_glyph_colour_inside, "SourceTextColorSamplerTests.Sample_ReadsTheRingAroundTheBoxAsBackgroundAndTheMajorityGlyphColourInside.json");
case!(
    black_text_in_awhite_balloon_over_ablack_panel_is_black_on_white,
    "TextLayersTests.BlackTextInAWhiteBalloon_OverABlackPanel_IsBlackOnWhite.json"
);
case!(
    capture_writes_an_outlined_source_in_whichever_of_its_colours_reads_on_the_card,
    "TextLayersTests.Capture_WritesAnOutlinedSourceInWhicheverOfItsColoursReadsOnTheCard.json"
);
case!(
    plain_black_text_has_no_outline,
    "TextLayersTests.PlainBlackText_HasNoOutline.json"
);
case!(
    plain_white_text_on_black_has_no_outline_its_counters_are_not_abody,
    "TextLayersTests.PlainWhiteTextOnBlack_HasNoOutline_ItsCountersAreNotABody.json"
);
case!(
    white_text_with_ablack_edge_is_white_with_ablack_outline,
    "TextLayersTests.WhiteTextWithABlackEdge_IsWhiteWithABlackOutline.json"
);
case!(
    white_text_with_ablack_edge_over_awhite_scene_is_still_white_with_ablack_outline,
    "TextLayersTests.WhiteTextWithABlackEdge_OverAWhiteScene_IsStillWhiteWithABlackOutline.json"
);
case!(
    white_text_with_adark_edge_on_an_orange_badge_keeps_the_white,
    "TextLayersTests.WhiteTextWithADarkEdge_OnAnOrangeBadge_KeepsTheWhite.json"
);
