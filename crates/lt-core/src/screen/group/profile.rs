//! The limits one grouping pass runs on.
//! A profile is chosen by the caller, never inferred from the text.

use serde::{Deserialize, Serialize};

/// How close in text size two lines must be before one can be the other wrapping.
/// Named because the interface profile quotes it.
pub const MIN_TEXT_SIZE_RATIO: f64 = 0.88;
/// The most leading two lines can have and still read as set solid, one under the other.
pub const SOLID_LINE_ADVANCE: f64 = 1.20;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupingProfile {
    pub tightly_set_min_text_size_ratio: f64,
    pub waive_length_test_when_set_solid: bool,
    pub solid_line_advance_when_wrapped: f64,
    pub refuse_speaker_line_starts: bool,
    pub takes_prose_evidence: bool,
}

/// Screenshots: speech bubbles open on one or two words and chat is set tight, so set-solid pairs
/// get more room and need no length evidence.
pub static GENERAL: GroupingProfile = GroupingProfile {
    tightly_set_min_text_size_ratio: 0.80,
    waive_length_test_when_set_solid: true,
    solid_line_advance_when_wrapped: 1.45,
    refuse_speaker_line_starts: true,
    takes_prose_evidence: true,
};

/// Interface text: the strict limits, with no relaxed evidence.
pub static INTERFACE: GroupingProfile = GroupingProfile {
    tightly_set_min_text_size_ratio: MIN_TEXT_SIZE_RATIO,
    waive_length_test_when_set_solid: false,
    solid_line_advance_when_wrapped: SOLID_LINE_ADVANCE,
    refuse_speaker_line_starts: false,
    takes_prose_evidence: false,
};

/// Strict limits that still refuse speaker line starts. A test control; the app uses GENERAL.
pub static REALTIME: GroupingProfile = GroupingProfile {
    tightly_set_min_text_size_ratio: MIN_TEXT_SIZE_RATIO,
    waive_length_test_when_set_solid: false,
    solid_line_advance_when_wrapped: SOLID_LINE_ADVANCE,
    refuse_speaker_line_starts: true,
    takes_prose_evidence: false,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
pub enum CaptureLayoutMode {
    #[default]
    General,
    Interface,
}

impl GroupingProfile {
    pub fn for_mode(mode: i32) -> &'static Self {
        if mode == CaptureLayoutMode::Interface as i32 {
            &INTERFACE
        } else {
            &GENERAL
        }
    }
}

pub struct CaptureLayoutPolicy;

impl CaptureLayoutPolicy {
    pub fn is_mode_selection_available() -> bool {
        false
    }
    pub fn for_application(_requested: i32) -> CaptureLayoutMode {
        CaptureLayoutMode::General
    }
}
