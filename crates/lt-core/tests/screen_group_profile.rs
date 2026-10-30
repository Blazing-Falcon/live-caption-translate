use lt_core::screen::group::profile::{
    CaptureLayoutMode, CaptureLayoutPolicy, GroupingProfile, GENERAL, INTERFACE,
    MIN_TEXT_SIZE_RATIO, REALTIME, SOLID_LINE_ADVANCE,
};

#[test]
fn capture_controls_have_separate_profiles_and_default_to_general() {
    assert!(std::ptr::eq(GroupingProfile::for_mode(1), &INTERFACE));
    assert!(std::ptr::eq(GroupingProfile::for_mode(0), &GENERAL));
    assert!(!std::ptr::eq(&INTERFACE, &GENERAL));
    for mode in [0, 1] {
        assert!(!std::ptr::eq(GroupingProfile::for_mode(mode), &REALTIME));
    }
    assert_eq!(CaptureLayoutMode::default(), CaptureLayoutMode::General);
    assert!(std::ptr::eq(
        GroupingProfile::for_mode(CaptureLayoutMode::default() as i32),
        &GENERAL
    ));
    assert!(std::ptr::eq(GroupingProfile::for_mode(99), &GENERAL));
}

#[test]
fn the_profile_carries_five_fields() {
    let GroupingProfile {
        tightly_set_min_text_size_ratio,
        waive_length_test_when_set_solid,
        solid_line_advance_when_wrapped,
        refuse_speaker_line_starts,
        takes_prose_evidence,
    } = GENERAL;
    assert_eq!(
        (
            tightly_set_min_text_size_ratio,
            waive_length_test_when_set_solid,
            solid_line_advance_when_wrapped,
            refuse_speaker_line_starts,
            takes_prose_evidence
        ),
        (0.8, true, 1.45, true, true)
    );
}

#[test]
fn only_general_takes_prose_evidence_and_relaxes_leading() {
    for profile in [&GENERAL, &INTERFACE, &REALTIME] {
        assert_eq!(
            profile.solid_line_advance_when_wrapped > SOLID_LINE_ADVANCE,
            profile.takes_prose_evidence
        );
    }
    assert!(GENERAL.takes_prose_evidence);
    assert_eq!(GENERAL.solid_line_advance_when_wrapped, 1.45);
    assert_eq!(
        INTERFACE.solid_line_advance_when_wrapped,
        SOLID_LINE_ADVANCE
    );
    assert_eq!(REALTIME.solid_line_advance_when_wrapped, SOLID_LINE_ADVANCE);
}

#[test]
fn general_and_realtime_refuse_speaker_line_starts() {
    assert!(GENERAL.refuse_speaker_line_starts);
    assert!(REALTIME.refuse_speaker_line_starts);
    assert!(!INTERFACE.refuse_speaker_line_starts);
}

#[test]
fn realtime_retains_its_recorded_values() {
    assert_eq!(REALTIME.tightly_set_min_text_size_ratio, 0.88);
    assert!(!REALTIME.waive_length_test_when_set_solid);
    assert_eq!(REALTIME.solid_line_advance_when_wrapped, 1.20);
}

#[test]
fn interface_retains_the_ordinary_thresholds() {
    assert_eq!(
        INTERFACE.tightly_set_min_text_size_ratio,
        MIN_TEXT_SIZE_RATIO
    );
    assert!(!INTERFACE.waive_length_test_when_set_solid);
    assert_eq!(
        INTERFACE.solid_line_advance_when_wrapped,
        SOLID_LINE_ADVANCE
    );
}

#[test]
fn application_ignores_saved_or_requested_mode() {
    for mode in [0, 1, 99] {
        assert!(!CaptureLayoutPolicy::is_mode_selection_available());
        assert_eq!(
            CaptureLayoutPolicy::for_application(mode),
            CaptureLayoutMode::General
        );
    }
    assert!(std::ptr::eq(GroupingProfile::for_mode(1), &INTERFACE));
}
