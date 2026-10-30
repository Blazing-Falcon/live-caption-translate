use lt_core::screen::raster::Raster;

#[test]
fn a_tall_crop_turn_filters_pixels_while_an_identity_resize_preserves_them() {
    let mut source = Raster {
        width: 7,
        height: 13,
        bgra: Vec::new(),
    };
    for y in 0..13 {
        for x in 0..7 {
            source.bgra.extend([
                (x * 17 + y * 83) as u8,
                (x * 31 + y * 53) as u8,
                (x * 71 + y * 37) as u8,
                255,
            ]);
        }
    }
    let unchanged = Raster::resize(&source.view(), 7, 13);
    assert_eq!(unchanged.bgra, source.bgra);
    let rotated = source.clockwise();
    assert_eq!((rotated.width, rotated.height), (13, 7));
    for ((x, y), expected) in [
        ((0, 0), [6, 5, 6, 255]),
        ((3, 6), [51, 155, 178, 255]),
        ((6, 12), [83, 50, 97, 255]),
        ((0, 12), [224, 123, 176, 255]),
        ((6, 0), [106, 187, 168, 255]),
    ] {
        let i = (x * 13 + 12 - y) * 4;
        assert_eq!(&rotated.bgra[i..i + 4], &expected);
    }
}
