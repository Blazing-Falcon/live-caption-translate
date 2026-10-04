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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stereo_is_averaged_and_surround_uses_center_without_lfe() {
        let mut out = Vec::new();
        downmix_interleaved(&[1.0, 0.0], 2, &mut out).unwrap();
        assert_eq!(out, [0.5]);
        out.clear();
        downmix_interleaved(&[0.0, 0.0, 1.0, 20.0, 30.0, 40.0], 6, &mut out).unwrap();
        assert!((out[0] - 0.707_106_77).abs() < 0.000_001);
        out.clear();
        downmix_interleaved(&[0.0, 0.0, 0.0, 20.0, 30.0, 40.0, 50.0, 60.0], 8, &mut out).unwrap();
        assert_eq!(out, [0.0]);
        assert!(downmix_interleaved(&[1.0], 2, &mut out).is_err());
        assert!(downmix_interleaved(&[], 0, &mut out).is_err());
    }
}
