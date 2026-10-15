//! The second-pass models read each of the 20 ASCEND clips whole, as the second pass reads a
//! line's window. A broken binding would not fail anything else: the second pass would quietly
//! keep the live text. Real inference requires LT_MODELS_DIR with the models and the test audio.

mod common;

use common::{errors, wav};
use lt_core::{config::AsrConfig, engines::OfflineAsr};
use lt_sherpa::{Qwen3Asr, SenseVoice};
use std::path::Path;

/// Character error rate over all clips, with and without a forced language.
fn error_rates(engine: &mut dyn OfflineAsr, audio: &Path, clips: &[common::Clip]) -> (f64, f64) {
    let (mut wrong, mut total) = (0, 0);
    let (mut forced_wrong, mut forced_total) = (0, 0);
    for clip in clips {
        let samples = wav(&audio.join(&clip.file));
        let heard = engine.decode(&samples, None).unwrap();
        let (e, n) = errors(&clip.reference, &heard.text);
        wrong += e;
        total += n;
        let forced = engine.decode(&samples, Some("zh")).unwrap();
        let (e, n) = errors(&clip.reference, &forced.text);
        forced_wrong += e;
        forced_total += n;
    }
    (
        wrong as f64 / total as f64,
        forced_wrong as f64 / forced_total as f64,
    )
}

#[test]
fn sensevoice_reads_whole_clips_accurately_with_and_without_a_forced_language() {
    let Some((models, audio, clips)) = common::ascend() else {
        return;
    };
    let mut engine = SenseVoice::new(
        models.join(lt_sherpa::SENSEVOICE_MODEL),
        &AsrConfig::default(),
    )
    .unwrap();
    let samples = wav(&audio.join(&clips[0].file));
    assert!(engine.decode(&samples, None).unwrap().lang.is_some());
    let (cer, forced_cer) = error_rates(&mut engine, &audio, &clips);
    eprintln!("sensevoice cer={cer:.3} forced_zh_cer={forced_cer:.3}");
    assert!(cer <= 0.21, "character error rate {cer:.3}");
    assert!(
        forced_cer <= 0.18,
        "character error rate with zh {forced_cer:.3}"
    );
}

#[test]
fn qwen3_reads_whole_clips_accurately_with_and_without_a_forced_language() {
    let Some((models, audio, clips)) = common::ascend() else {
        return;
    };
    let mut engine =
        Qwen3Asr::new(models.join(lt_sherpa::QWEN3_MODEL), &AsrConfig::default()).unwrap();
    let (cer, forced_cer) = error_rates(&mut engine, &audio, &clips);
    eprintln!("qwen3 cer={cer:.3} forced_zh_cer={forced_cer:.3}");
    assert!(cer <= 0.17, "character error rate {cer:.3}");
    assert!(
        forced_cer <= 0.17,
        "character error rate with zh {forced_cer:.3}"
    );
}
