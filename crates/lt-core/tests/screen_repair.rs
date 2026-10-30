use lt_core::screen::{
    repair::{find, Bounds},
    ImageView,
};

fn box_at(left: f32, width: f32) -> Bounds {
    Bounds {
        left,
        top: 20.0,
        right: left + width,
        bottom: 44.0,
    }
}

fn page(background: [u8; 3], gap: std::ops::Range<usize>, grey: bool) -> Vec<u8> {
    let mut pixels = [background[2], background[1], background[0], 255].repeat(400 * 80);
    for x in (20..320).step_by(6) {
        if gap.contains(&x) {
            continue;
        }
        let ink = if grey {
            if (140..200).contains(&x) {
                [190, 170, 150, 255]
            } else {
                [178, 176, 176, 255]
            }
        } else {
            [220, 160, 120, 255]
        };
        for stroke in 0..2 {
            for y in 20..40 {
                pixels[(y * 400 + x + stroke) * 4..][..4].copy_from_slice(&ink);
            }
        }
    }
    pixels
}

fn repairs(pixels: &[u8], boxes: &[Bounds]) -> Vec<lt_core::screen::repair::Repair> {
    find(&ImageView::new(400, 80, 1600, pixels).unwrap(), boxes)
}

#[test]
fn a_broken_colored_row_is_rejoined() {
    let result = repairs(
        &page([24, 24, 32], 0..0, false),
        &[box_at(20.0, 120.0), box_at(200.0, 120.0)],
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].owners, [0, 1]);
    assert!(result[0].bounds.left <= 20.0 && result[0].bounds.right >= 320.0);
}

#[test]
fn a_grey_row_with_hued_missing_glyphs_is_rejoined() {
    let result = repairs(
        &page([24, 24, 32], 0..0, true),
        &[box_at(20.0, 120.0), box_at(200.0, 120.0)],
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].owners, [0, 1]);
}

#[test]
fn pale_background_is_not_repaired() {
    assert!(repairs(
        &page([240, 240, 244], 0..0, false),
        &[box_at(20.0, 120.0), box_at(200.0, 120.0)]
    )
    .is_empty());
}

#[test]
fn separate_items_are_not_joined_across_real_space() {
    for repair in repairs(
        &page([24, 24, 32], 150..198, false),
        &[box_at(20.0, 120.0), box_at(200.0, 120.0)],
    ) {
        assert_eq!(repair.owners.len(), 1);
    }
}

#[test]
fn a_covered_row_is_left_alone() {
    assert!(repairs(&page([24, 24, 32], 0..0, false), &[box_at(18.0, 304.0)]).is_empty());
}

#[test]
fn colored_ink_needs_a_substantial_detection() {
    assert!(repairs(
        &page([24, 24, 32], 0..0, false),
        &[box_at(20.0, 40.0), box_at(200.0, 40.0)]
    )
    .is_empty());
}

#[test]
fn a_row_does_not_reach_into_a_picture_beside_it() {
    let mut pixels = page([24, 24, 32], 0..0, false);
    for x in 335..396 {
        for y in 14..50 {
            pixels[(y * 400 + x) * 4..][..4].copy_from_slice(&[90, 120, 150, 255]);
        }
    }
    let result = repairs(&pixels, &[box_at(20.0, 120.0), box_at(200.0, 120.0)]);
    assert!(!result.is_empty());
    assert!(result.iter().all(|r| r.bounds.right < 360.0));
}

#[test]
fn an_icon_bridging_two_rows_never_joins_them() {
    let mut pixels = [32, 24, 24, 255].repeat(640 * 120);
    let ink = [220, 160, 120, 255];
    for x in 20..44 {
        for y in 20..84 {
            pixels[(y * 640 + x) * 4..][..4].copy_from_slice(&ink);
        }
    }
    for row in [20, 60] {
        for x in (60..560).step_by(6) {
            for stroke in 0..2 {
                for y in row..row + 20 {
                    pixels[(y * 640 + x + stroke) * 4..][..4].copy_from_slice(&ink);
                }
            }
        }
    }
    let boxes = [
        Bounds {
            left: 60.0,
            top: 20.0,
            right: 260.0,
            bottom: 40.0,
        },
        Bounds {
            left: 60.0,
            top: 60.0,
            right: 260.0,
            bottom: 80.0,
        },
    ];
    let result = find(&ImageView::new(640, 120, 2560, &pixels).unwrap(), &boxes);
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|r| r.owners.len() == 1));
    let mut owners: Vec<_> = result
        .iter()
        .flat_map(|r| r.owners.iter().copied())
        .collect();
    owners.sort();
    assert_eq!(owners, [0, 1]);
}
