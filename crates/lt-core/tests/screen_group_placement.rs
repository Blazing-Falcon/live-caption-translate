use lt_core::screen::{
    geom::RectF,
    group::profile::{CaptureLayoutMode, CaptureLayoutPolicy},
    line::Line,
    placement::{intent_for, LayoutIntent},
};

#[test]
fn general_placement_resets_the_whole_group() {
    let block = Line {
        source_line_bounds: Some(vec![
            RectF::new(10.0, 10.0, 200.0, 28.0),
            RectF::new(10.0, 40.0, 180.0, 28.0),
        ]),
        ..Line::new("SOURCE", RectF::new(10.0, 10.0, 200.0, 60.0))
    };
    assert_eq!(intent_for(&block), LayoutIntent::GroupReflow);
}

#[test]
fn general_placement_leaves_one_line_on_the_ordinary_path() {
    let block = Line::new("SOURCE", RectF::new(10.0, 10.0, 200.0, 28.0));
    assert_eq!(intent_for(&block), LayoutIntent::Single);
}

#[test]
fn an_unknown_requested_mode_reaches_the_default_placement() {
    assert_eq!(
        CaptureLayoutPolicy::for_application(99),
        CaptureLayoutMode::General
    );
    let block = Line {
        source_line_bounds: Some(vec![
            RectF::new(10.0, 10.0, 200.0, 28.0),
            RectF::new(10.0, 40.0, 180.0, 28.0),
        ]),
        ..Line::new("SOURCE", RectF::new(10.0, 10.0, 200.0, 60.0))
    };
    assert_eq!(intent_for(&block), LayoutIntent::GroupReflow);
}
