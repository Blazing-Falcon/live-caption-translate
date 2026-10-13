//! The second pass keeps only the part of its window that belongs to the live clause. Cases
//! come from real decodes, where the window holds the ends of the neighboring clauses.

use lt_core::refine::trim;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    live: String,
    window: String,
    expected: Option<String>,
}

#[test]
fn the_window_is_trimmed_to_the_live_clause_without_neighboring_words() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/second-pass-trim.json")).unwrap();
    for case in fixture.cases {
        let chosen = trim(&case.live, &case.window).map(|trimmed| trimmed.text);
        assert_eq!(chosen, case.expected, "live {:?}", case.live);
    }
}
