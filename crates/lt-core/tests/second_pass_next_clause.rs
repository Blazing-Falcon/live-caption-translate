//! The second pass must not carry the next clause into this one's text.
//!
//! A window runs past its clause, so the offline recognizer hears the start of the next clause
//! too. When that start is already known from the live text, cutting the window there keeps the
//! next clause's words out of `trim`'s reach. The cases are the 169 windows of a recorded
//! session, with the text each one produced.

use lt_core::refine::{cut_at_next_clause, trim};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: u64,
    live: String,
    window: String,
    /// The clause after this one. Absent for the last clause of a session, which has none.
    next: Option<String>,
    expected: Option<String>,
}

#[test]
fn cutting_at_the_next_clause_keeps_its_words_out_of_the_trimmed_text() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/second-pass-next-clause.json")).unwrap();
    let mut changed = 0;
    let mut regressed = Vec::new();
    for case in &fixture.cases {
        let cut = case
            .next
            .as_deref()
            .and_then(|next| cut_at_next_clause(&case.window, next));
        let window = cut.unwrap_or(&case.window);
        let chosen = trim(&case.live, window).map(|trimmed| trimmed.text);
        if chosen != case.expected {
            regressed.push((
                case.id,
                &case.live,
                &case.window,
                chosen.clone(),
                &case.expected,
            ));
        }
        if cut.is_some() {
            changed += 1;
        }
    }
    // The cut must fire on real windows, or it is dead code.
    assert!(
        changed > 20,
        "the cut fired on {changed} of {} windows",
        fixture.cases.len()
    );
    assert!(
        regressed.is_empty(),
        "{} windows changed their text: {regressed:#?}",
        regressed.len()
    );
}

#[test]
fn a_connective_alone_does_not_cut() {
    // 然后 also occurs inside a clause, so a window that ends with it says nothing about where
    // the next clause begins.
    let window = "他先走了然后";
    assert_eq!(cut_at_next_clause(window, "然后我们再说"), None);
}

#[test]
fn a_one_character_match_does_not_cut() {
    let window = "我今天去公园";
    assert_eq!(cut_at_next_clause(window, "我明天去"), None);
}

#[test]
fn a_cut_needs_a_clause_boundary_before_the_opening() {
    // The window ends with the next clause's opening, but nothing separates it from this
    // clause's own words: the match is a coincidence inside a word, not a boundary.
    let window = "我今天去公园玩";
    assert_eq!(cut_at_next_clause(window, "公园玩得很开心"), None);
    // With a mark between them it is a boundary.
    assert_eq!(
        cut_at_next_clause("我今天去公园。明天去学校", "明天去学校"),
        Some("我今天去公园。")
    );
}
