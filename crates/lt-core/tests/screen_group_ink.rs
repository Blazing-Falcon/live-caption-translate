use lt_core::screen::{geom::RectF, ink, ImageView};

#[test]
fn ink_evidence_abstains_on_empty_or_non_text_geometry() {
    let pixels = vec![255; 200 * 60 * 4];
    let image = ImageView::new(200, 60, 800, &pixels).unwrap();
    assert_eq!(
        ink::measure(&image, RectF::new(0.0, 0.0, 200.0, 20.0)),
        None
    );
    assert_eq!(ink::measure(&image, RectF::new(0.0, 0.0, 10.0, 20.0)), None);
    assert_eq!(
        ink::measure(&image, RectF::new(500.0, 0.0, 200.0, 20.0)),
        None
    );
}
