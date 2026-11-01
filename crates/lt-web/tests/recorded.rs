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
