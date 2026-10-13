//! Streaming X-ASR on the 20 ASCEND clips, fed in 100 ms pieces like live capture. Real
//! inference requires LT_MODELS_DIR with the streaming model and the test audio.

mod common;

use common::{errors, wav};
use lt_core::{config::AsrConfig, engines::StreamingAsr};
use lt_sherpa::XAsrStream;

/// The clause tracker cuts on tokens it has seen: a later result must only add tokens.
#[test]
fn streaming_tokens_only_grow_and_the_text_stays_accurate() {
    let Some((models, audio, clips)) = common::ascend() else {
        return;
    };
    let settings = AsrConfig::default();
    let mut engine = XAsrStream::new(
        models.join(lt_sherpa::xasr_stream_model(settings.chunk_ms)),
        &settings,
    )
    .unwrap();
    let (mut wrong, mut total) = (0, 0);
    for clip in &clips {
        let samples = wav(&audio.join(&clip.file));
        let mut seen: Vec<String> = Vec::new();
        for piece in samples.chunks(1_600) {
            engine.accept(piece).unwrap();
            let result = engine.result().unwrap();
            assert!(
                result.tokens.tokens.starts_with(&seen),
                "{}: emitted tokens changed",
                clip.file
            );
            assert_eq!(
                result.tokens.tokens.len(),
                result.tokens.timestamps.len(),
                "one start time per token"
            );
            seen = result.tokens.tokens;
        }
        let last = engine.finish().unwrap();
        assert!(last.tokens.tokens.starts_with(&seen));
        let (e, n) = errors(&clip.reference, &last.tokens.tokens.concat());
        wrong += e;
        total += n;
    }
    let cer = wrong as f64 / total as f64;
    eprintln!("xasr_stream cer={cer:.3}");
    assert!(cer <= 0.12, "character error rate {cer:.3}");
}
