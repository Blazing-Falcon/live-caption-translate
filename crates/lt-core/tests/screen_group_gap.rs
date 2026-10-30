use lt_core::screen::group::gap::{SameLineGapThreshold, FALLBACK};

const TWO_KINDS: [f64; 7] = [0.10, 0.14, 0.18, 0.22, 0.60, 0.64, 0.68];

fn falls_back(gaps: &[f64]) {
    let result = SameLineGapThreshold::estimate(gaps);
    assert!(!result.adaptive);
    assert_eq!(result.value, FALLBACK);
}

#[test]
fn draws_the_line_between_two_kinds_of_space() {
    let threshold = SameLineGapThreshold::estimate(&TWO_KINDS);
    assert!(threshold.adaptive, "{}", threshold.reason);
    assert_eq!((threshold.value * 1000.0).round_ties_even(), 410.0);
}

#[test]
fn falls_back_instead_of_opening_the_gate_wide() {
    falls_back(&[0.40, 0.44, 0.46, 0.48, 1.30, 1.42, 1.55]);
}

#[test]
fn refuses_to_split_one_population() {
    falls_back(&[0.10, 0.13, 0.16, 0.19, 0.22, 0.26, 0.30]);
}

#[test]
fn refuses_to_split_a_menu_into_words_and_items() {
    falls_back(&[0.56, 0.63, 0.67, 0.70, 0.71, 0.72, 0.76]);
}

#[test]
fn falls_back_when_there_is_too_little_to_go_on() {
    falls_back(&[0.12, 0.70]);
}

#[test]
fn falls_back_on_a_capture_with_no_gaps() {
    falls_back(&[]);
}

#[test]
fn falls_back_when_the_answer_is_outside_real_spacing() {
    falls_back(&[0.02, 0.03, 0.04, 0.05, 0.06, 0.07, 0.90]);
}

#[test]
fn ignores_distances_that_are_not_spacing() {
    let mut gaps = TWO_KINDS.to_vec();
    gaps.extend([8.0, 12.0, 20.0]);
    assert_eq!(
        (SameLineGapThreshold::estimate(&gaps).value * 1000.0).round_ties_even(),
        (SameLineGapThreshold::estimate(&TWO_KINDS).value * 1000.0).round_ties_even()
    );
}

#[test]
fn gives_the_same_answer_whatever_order_gaps_arrive_in() {
    let mut reversed = TWO_KINDS;
    reversed.reverse();
    assert_eq!(
        (SameLineGapThreshold::estimate(&reversed).value * 1e6).round_ties_even(),
        (SameLineGapThreshold::estimate(&TWO_KINDS).value * 1e6).round_ties_even()
    );
}
