const SHIFT: usize = 5;
const LEVELS: usize = 256 >> SHIFT;
const BUCKETS: usize = LEVELS * LEVELS * LEVELS;
pub struct DominantColorVote {
    count: [u32; BUCKETS],
    sums: [[u64; 3]; BUCKETS],
}
impl Default for DominantColorVote {
    fn default() -> Self {
        Self {
            count: [0; BUCKETS],
            sums: [[0; 3]; BUCKETS],
        }
    }
}
impl DominantColorVote {
    pub fn add(&mut self, rgb: [u8; 3]) {
        let key = (((rgb[0] as usize >> SHIFT) * LEVELS + (rgb[1] as usize >> SHIFT)) * LEVELS)
            + (rgb[2] as usize >> SHIFT);
        self.count[key] += 1;
        for (c, v) in rgb.iter().enumerate() {
            self.sums[key][c] += u64::from(*v);
        }
    }
    pub fn dominant_with_share(&self) -> Option<([u8; 3], f64)> {
        let mut best = None;
        let mut total = 0u64;
        for (key, count) in self.count.iter().enumerate() {
            total += u64::from(*count);
            if *count > 0 && best.is_none_or(|b| *count > self.count[b]) {
                best = Some(key);
            }
        }
        best.map(|b| {
            let n = u64::from(self.count[b]);
            (
                [
                    (self.sums[b][0] / n) as u8,
                    (self.sums[b][1] / n) as u8,
                    (self.sums[b][2] / n) as u8,
                ],
                n as f64 / total as f64,
            )
        })
    }
    pub fn dominant(&self) -> Option<[u8; 3]> {
        self.dominant_with_share().map(|(color, _)| color)
    }
}
