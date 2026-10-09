//! Capture's front-channel mix; LFE and surrounds are deliberately excluded.

use lt_core::error::{Error, Result};

pub fn downmix_interleaved(input: &[f32], channels: u16, output: &mut Vec<f32>) -> Result<()> {
    let channels = usize::from(channels);
    if channels == 0 || !input.len().is_multiple_of(channels) {
        return Err(Error::Engine(
            "Audio block does not contain complete channel frames".into(),
        ));
    }
    output.reserve(input.len() / channels);
    for frame in input.chunks_exact(channels) {
        let mono = if channels == 1 {
            frame[0]
        } else {
            let fronts = (frame[0] + frame[1]) * 0.5;
            if matches!(channels, 6 | 8) {
                fronts + frame[2] * std::f32::consts::FRAC_1_SQRT_2
            } else {
                fronts
            }
        };
        output.push(mono);
    }
    Ok(())
}
