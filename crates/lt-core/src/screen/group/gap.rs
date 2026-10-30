use serde::Serialize;

pub const FALLBACK: f64 = 0.45;
const SAMPLE_FLOOR: f64 = -1.0;
const SAMPLE_CEILING: f64 = 2.0;
const MINIMUM_SAMPLES: usize = 6;
const MINIMUM_HALF_SIZE: usize = 2;
const MINIMUM_SEPARATION: f64 = 0.15;
const MINIMUM_THRESHOLD: f64 = 0.25;
const MAXIMUM_THRESHOLD: f64 = 0.55;

#[derive(Clone, Debug, Serialize)]
pub struct SameLineGapThreshold {
    pub value: f64,
    pub adaptive: bool,
    pub reason: &'static str,
}

impl SameLineGapThreshold {
    pub fn estimate(gaps: &[f64]) -> Self {
        let mut samples: Vec<_> = gaps
            .iter()
            .copied()
            .filter(|&gap| (SAMPLE_FLOOR..=SAMPLE_CEILING).contains(&gap))
            .collect();
        samples.sort_by(f64::total_cmp);
        if samples.len() < MINIMUM_SAMPLES {
            return Self::fallback_with("too few gaps");
        }
        let Some(split) = best_split(&samples) else {
            return Self::fallback_with("no two-sided split");
        };
        let tightest = samples[split - 1];
        let loosest = samples[split];
        if loosest - tightest < MINIMUM_SEPARATION {
            return Self::fallback_with("halves not separated");
        }
        let threshold = (tightest + loosest) / 2.0;
        if !(MINIMUM_THRESHOLD..=MAXIMUM_THRESHOLD).contains(&threshold) {
            Self::fallback_with("outside the believable range")
        } else {
            Self {
                value: threshold,
                adaptive: true,
                reason: "measured on this capture",
            }
        }
    }

    fn fallback_with(reason: &'static str) -> Self {
        Self {
            value: FALLBACK,
            adaptive: false,
            reason,
        }
    }
}

fn best_split(sorted: &[f64]) -> Option<usize> {
    let mut best = None;
    let mut best_spread = f64::INFINITY;
    for split in MINIMUM_HALF_SIZE..=sorted.len() - MINIMUM_HALF_SIZE {
        let spread = spread(sorted, 0, split) + spread(sorted, split, sorted.len());
        if spread >= best_spread {
            continue;
        }
        best_spread = spread;
        best = Some(split);
    }
    best
}

fn spread(sorted: &[f64], from: usize, to: usize) -> f64 {
    let mut sum = 0.0;
    for &value in &sorted[from..to] {
        sum += value;
    }
    let mean = sum / (to - from) as f64;
    let mut spread = 0.0;
    for &value in &sorted[from..to] {
        spread += (value - mean) * (value - mean);
    }
    spread
}
