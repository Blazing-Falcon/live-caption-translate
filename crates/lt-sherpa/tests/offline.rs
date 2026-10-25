//! Offline X-ASR (the second pass) on the 20 ASCEND clips, each decoded whole. Real inference
//! requires LT_MODELS_DIR with the offline model and the test audio.

mod common;

use common::{errors, wav};
use lt_core::{config::AsrConfig, engines::OfflineAsr};

/// The second pass replaces the live text, so it must be at least as accurate and spelled the
/// same way: no space next to a Chinese character unless an English word is on the other side.
#[test]
fn offline_text_is_accurate_and_spaced_like_the_live_text() {
    let Some((models, audio, clips)) = common::ascend() else {
        return;
    };
    let mut engine = lt_sherpa::XAsrOffline::new(
        models.join(lt_sherpa::XASR_OFFLINE_MODEL),
        &AsrConfig::default(),
    )
    .unwrap();
    let (mut wrong, mut total) = (0, 0);
    for clip in &clips {
        let text = engine
            .decode(&wav(&audio.join(&clip.file)), None)
            .unwrap()
            .text;
        let chars: Vec<char> = text.chars().collect();
        for (index, pair) in chars.windows(3).enumerate() {
            if pair[1] == ' ' {
                assert!(
                    pair[0].is_ascii_alphanumeric() || pair[2].is_ascii_alphanumeric(),
                    "{}: space between non-English characters at {index}: {text}",
                    clip.file
                );
            }
        }
        let (e, n) = errors(&clip.reference, &text);
        wrong += e;
        total += n;
    }
    let cer = wrong as f64 / total as f64;
    eprintln!("xasr_offline cer={cer:.3}");
    assert!(cer <= 0.10, "character error rate {cer:.3}");
    // A window cut short by a break must not reach the encoder's exception, which aborts.
    for samples in [0, 800] {
        assert!(engine.decode(&vec![0.0; samples], None).is_ok());
    }
}
