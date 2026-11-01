use lt_core::config::TranslateEngine;

#[test]
fn recorded_answers_preserve_count_and_text() {
    for (engine, raw, expected) in [
        (
            TranslateEngine::Microsoft,
            include_str!("fixtures/microsoft/public.json"),
            include_str!("fixtures/microsoft/public.expected.json"),
        ),
        (
            TranslateEngine::Google,
            include_str!("fixtures/google/public.json"),
            include_str!("fixtures/google/public.expected.json"),
        ),
    ] {
        let expected: Vec<String> = serde_json::from_str(expected).unwrap();
        assert_eq!(expected.len(), 3);
        assert_eq!(lt_web::parse_response(engine, raw).unwrap(), expected);
        assert!(lt_web::parse_response(engine, "{}").is_err());
    }
}

#[test]
fn google_alternate_recording_matches_position_tags() {
    let expected: Vec<String> =
        serde_json::from_str(include_str!("fixtures/google_alt/public.expected.json")).unwrap();
    assert_eq!(expected.len(), 3);
    assert_eq!(
        lt_web::parse_google_alt(include_str!("fixtures/google_alt/public.json"), 3).unwrap(),
        expected
    );
    assert!(lt_web::parse_google_alt("{}", 3).is_err());
}
