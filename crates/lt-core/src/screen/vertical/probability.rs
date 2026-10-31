//! Splits merged columns using the detector's probability map.

/// How much ink a strip must carry, against the heaviest in the box, to be a column of its own
/// rather than the reading beside one.
const COLUMN_INK_SHARE: f64 = 0.67;
/// The least a strip may fill of its own rectangle and still be writing. Scribbled artwork lights
/// the map thinly and over an area.
const COLUMN_FILL: f64 = 0.3;
/// A position across a strip is part of its column when it is on for this much of its length.
const CORE_LENGTH: f64 = 0.3;
/// Blobs smaller than this, in map pixels, are specks.
const MINIMUM_BLOB: usize = 12;

pub struct DetectorProbability<'a> {
    pub values: &'a [f32],
    pub width: usize,
    pub height: usize,
    pub scale_x: f32,
    pub scale_y: f32,
    pub threshold: f32,
    pub score_threshold: f32,
    pub unclip_ratio: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub bounds: [i32; 4],
    pub kernel: [i32; 4],
}
#[derive(Clone, Copy)]
pub(super) struct Blob {
    pub id: usize,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub count: usize,
    pub sum: f64,
}
impl Blob {
    pub fn width(self) -> i32 {
        self.right - self.left + 1
    }
    pub fn height(self) -> i32 {
        self.bottom - self.top + 1
    }
    fn join(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
            ..self
        }
    }
}

pub(super) fn blobs(
    map: &[f32],
    width: usize,
    height: usize,
    threshold: f32,
    area: [i32; 4],
) -> (Vec<Blob>, Vec<usize>) {
    let [x0, y0, x1, y1] = area;
    let region_width = (x1 - x0) as usize;
    let mut labels = vec![0; region_width * (y1 - y0) as usize];
    let label_index = |x: i32, y: i32| (y - y0) as usize * region_width + (x - x0) as usize;
    let mut result = Vec::new();
    let mut stack = Vec::new();
    let mut id = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let start = label_index(x, y);
            if labels[start] != 0 || map[y as usize * width + x as usize] <= threshold {
                continue;
            }
            id += 1;
            let mut blob = Blob {
                id,
                left: i32::MAX,
                top: i32::MAX,
                right: i32::MIN,
                bottom: i32::MIN,
                count: 0,
                sum: 0.0,
            };
            labels[start] = id;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (
                    x0 + (i % region_width) as i32,
                    y0 + (i / region_width) as i32,
                );
                blob.left = blob.left.min(x);
                blob.right = blob.right.max(x);
                blob.top = blob.top.min(y);
                blob.bottom = blob.bottom.max(y);
                blob.count += 1;
                blob.sum += f64::from(map[y as usize * width + x as usize]);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < x0
                            || ny < y0
                            || nx >= x1
                            || ny >= y1
                            || nx >= width as i32
                            || ny >= height as i32
                        {
                            continue;
                        }
                        let n = label_index(nx, ny);
                        if labels[n] != 0 || map[ny as usize * width + nx as usize] <= threshold {
                            continue;
                        }
                        labels[n] = id;
                        stack.push(n);
                    }
                }
            }
            result.push(blob);
        }
    }
    (result, labels)
}

impl DetectorProbability<'_> {
    pub fn columns_in(&self, left: i32, top: i32, right: i32, bottom: i32) -> Vec<Column> {
        let x0 = ((left as f32 * self.scale_x).floor() as i32).max(0);
        let y0 = ((top as f32 * self.scale_y).floor() as i32).max(0);
        let x1 = ((right as f32 * self.scale_x).ceil() as i32).min(self.width as i32);
        let y1 = ((bottom as f32 * self.scale_y).ceil() as i32).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return Vec::new();
        }
        let mut strips: Vec<Blob> = Vec::new();
        for blob in blobs(
            self.values,
            self.width,
            self.height,
            self.threshold,
            [x0, y0, x1, y1],
        )
        .0
        {
            if blob.count < MINIMUM_BLOB
                || blob.sum / (blob.count as f64) < f64::from(self.score_threshold)
            {
                continue;
            }
            let mut joined = blob;
            while let Some(at) = strips
                .iter()
                .position(|s| joined.left <= s.right && joined.right >= s.left)
            {
                joined = joined.join(strips.remove(at));
            }
            strips.push(joined);
        }
        let mut columns = Vec::new();
        for s in strips {
            let (mut core, mut on) = (0, 0);
            for x in s.left..=s.right {
                let count = (s.top..=s.bottom)
                    .filter(|y| self.values[*y as usize * self.width + x as usize] > self.threshold)
                    .count();
                on += count;
                if count as f64 >= f64::from(s.height()) * CORE_LENGTH {
                    core += 1;
                }
            }
            let ink = on as f64 / f64::from(s.height());
            if core > 0
                && f64::from(s.height()) >= f64::from(core) * 1.5
                && ink / f64::from(s.width()) >= COLUMN_FILL
            {
                columns.push((s, ink));
            }
        }
        if columns.len() < 2 {
            return Vec::new();
        }
        let heaviest = columns
            .iter()
            .map(|(_, ink)| *ink)
            .fold(f64::NEG_INFINITY, f64::max);
        columns.retain(|(_, ink)| *ink >= heaviest * COLUMN_INK_SHARE);
        if columns.len() < 2 {
            return Vec::new();
        }
        columns.sort_by_key(|(s, _)| s.left);
        columns
            .into_iter()
            .map(|(s, _)| {
                let offset = s.width() as f32 * s.height() as f32 * self.unclip_ratio
                    / (2.0 * (s.width() + s.height()) as f32)
                    + 1.0;
                Column {
                    bounds: [
                        left.max(((s.left as f32 - offset) / self.scale_x) as i32),
                        top.max(((s.top as f32 - offset) / self.scale_y) as i32),
                        right.min(((s.right as f32 + 1.0 + offset) / self.scale_x) as i32),
                        bottom.min(((s.bottom as f32 + 1.0 + offset) / self.scale_y) as i32),
                    ],
                    kernel: [
                        (s.left as f32 / self.scale_x) as i32,
                        (s.top as f32 / self.scale_y) as i32,
                        ((s.right + 1) as f32 / self.scale_x) as i32,
                        ((s.bottom + 1) as f32 / self.scale_y) as i32,
                    ],
                }
            })
            .collect()
    }
}
