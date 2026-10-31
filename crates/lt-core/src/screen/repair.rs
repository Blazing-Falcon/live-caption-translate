//! Finds glyphs the detector stopped short of at the ends of a box and widens the box to cover
//! them.

use super::ImageView;

/// How far past the outermost box a row may reach, in line heights. It is a budget for a glyph or
/// two, so a row does not walk into a nearby picture.
const REACH_LIMIT: f64 = 1.5;

#[derive(Clone, Copy, Debug, Default)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    fn width(self) -> f32 {
        self.right - self.left
    }
    fn height(self) -> f32 {
        self.bottom - self.top
    }
    fn contains(self, x: i32, y: i32) -> bool {
        x as f32 >= self.left
            && (x as f32) < self.right
            && y as f32 >= self.top
            && (y as f32) < self.bottom
    }
    fn intersects(self, other: Self) -> bool {
        self.right.min(other.right) > self.left.max(other.left)
            && self.bottom.min(other.bottom) > self.top.max(other.top)
    }
}

#[derive(Debug)]
pub struct Repair {
    pub owners: Vec<usize>,
    pub bounds: Bounds,
}

pub fn find(source: &ImageView<'_>, boxes: &[Bounds]) -> Vec<Repair> {
    if boxes.is_empty() {
        return Vec::new();
    }
    let pixel = |x: i32, y: i32| -> [i32; 3] {
        let row = source.row(y as u32);
        let i = x as usize * 4;
        [
            i32::from(row[i + 2]),
            i32::from(row[i + 1]),
            i32::from(row[i]),
        ]
    };
    let mut histogram = [0usize; 4096];
    let step = (source.width.max(source.height) / 512).max(1) as usize;
    let mut samples = 0;
    for y in (0..source.height).step_by(step) {
        for x in (0..source.width).step_by(step) {
            let c = pixel(x as i32, y as i32);
            histogram[((c[0] >> 4) * 256 + (c[1] >> 4) * 16 + (c[2] >> 4)) as usize] += 1;
            samples += 1;
        }
    }
    let mut mode = 0;
    for i in 1..histogram.len() {
        if histogram[i] > histogram[mode] {
            mode = i;
        }
    }
    let bg = [
        ((mode >> 8) * 16 + 8) as i32,
        (((mode >> 4) & 15) * 16 + 8) as i32,
        ((mode & 15) * 16 + 8) as i32,
    ];
    let high = *bg.iter().max().unwrap();
    if (histogram[mode] as f64) < samples as f64 * 0.6
        || high > 96
        || high - bg.iter().min().unwrap() > 32
    {
        return Vec::new();
    }
    let ink = |x, y| pixel(x, y).iter().zip(bg).any(|(c, b)| (c - b).abs() > 40);
    let colored = |x, y| {
        let c = pixel(x, y);
        let high = *c.iter().max().unwrap();
        let low = *c.iter().min().unwrap();
        high - low >= 32 && low * 2 >= high
    };
    let width = source.width as i32;
    let image_height = source.height as i32;
    let mut repairs = Vec::new();
    let mut claimed = vec![false; boxes.len()];
    let mut seeds: Vec<_> = (0..boxes.len()).collect();
    seeds.sort_by(|&a, &b| boxes[b].width().total_cmp(&boxes[a].width()));
    for seed in seeds {
        if claimed[seed] {
            continue;
        }
        let mut owners: Vec<_> = (0..boxes.len())
            .filter(|&i| {
                !claimed[i]
                    && boxes[i].bottom.min(boxes[seed].bottom) - boxes[i].top.max(boxes[seed].top)
                        >= boxes[i].height().min(boxes[seed].height()) * 0.6
            })
            .collect();
        owners.sort_by(|&a, &b| boxes[a].left.total_cmp(&boxes[b].left));
        for &i in &owners {
            claimed[i] = true;
        }
        if owners.is_empty() {
            continue;
        }
        let mut lines = vec![vec![owners[0]]];
        for k in 1..owners.len() {
            let previous = boxes[owners[k - 1]];
            let next = boxes[owners[k]];
            let top = (previous.top.min(next.top).floor() as i32).max(0);
            let bottom = (previous.bottom.max(next.bottom).ceil() as i32).min(image_height);
            let from = (previous.right.ceil() as i32).max(0);
            let to = (next.left.floor() as i32).min(width);
            let (mut blank, mut widest) = (0, 0);
            for x in from..to {
                blank = if (top..bottom).any(|y| ink(x, y)) {
                    0
                } else {
                    blank + 1
                };
                widest = widest.max(blank);
            }
            if widest as f32 > boxes[seed].height() {
                lines.push(Vec::new());
            }
            lines.last_mut().unwrap().push(owners[k]);
        }
        for line in lines {
            let top = (line
                .iter()
                .map(|&i| boxes[i].top)
                .fold(f32::INFINITY, f32::min)
                .floor() as i32)
                .max(0);
            let bottom = (line
                .iter()
                .map(|&i| boxes[i].bottom)
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil() as i32)
                .min(image_height);
            let box_left = (line
                .iter()
                .map(|&i| boxes[i].left)
                .fold(f32::INFINITY, f32::min)
                .floor() as i32)
                .max(0);
            let box_right = (line
                .iter()
                .map(|&i| boxes[i].right)
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil() as i32)
                .min(width);
            if bottom - top < 8 || box_right - box_left < 8 {
                continue;
            }
            let mut inked = vec![false; width as usize];
            let (mut ink_top, mut ink_bottom, mut count) = (bottom, top, 0);
            for y in top..bottom {
                for x in box_left..box_right {
                    if !ink(x, y) {
                        continue;
                    }
                    inked[x as usize] = true;
                    count += 1;
                    ink_top = ink_top.min(y);
                    ink_bottom = ink_bottom.max(y + 1);
                }
            }
            let height = ink_bottom - ink_top;
            if height < 18 {
                continue;
            }
            let reach = height.max(2);
            let span = reach.max((f64::from(height) * REACH_LIMIT) as i32);
            let floor = (box_left - span).max(0);
            let ceiling = (box_right + span).min(width);
            for y in top..bottom {
                for x in (floor..box_left).chain(box_right..ceiling) {
                    if ink(x, y) {
                        inked[x as usize] = true;
                    }
                }
            }
            let mut left = box_left;
            let mut blank = 0;
            while left > floor && blank < reach {
                blank = if inked[(left - 1) as usize] {
                    0
                } else {
                    blank + 1
                };
                left -= 1;
            }
            while left < box_left && !inked[left as usize] {
                left += 1;
            }
            let mut right = box_right;
            blank = 0;
            while right < ceiling && blank < reach {
                blank = if inked[right as usize] { 0 } else { blank + 1 };
                right += 1;
            }
            while right > box_right && !inked[(right - 1) as usize] {
                right -= 1;
            }
            let off_the_line = |from: i32, to: i32| {
                if to - from < 2 {
                    return false;
                }
                let margin = (height / 2).max(2);
                let above = (ink_top - margin).max(0);
                let below = (ink_bottom + margin).min(image_height);
                let (mut columns, mut stray) = (0, 0);
                for x in from..to {
                    if !inked[x as usize] {
                        continue;
                    }
                    columns += 1;
                    if (above..below).any(|y| (y < ink_top || y >= ink_bottom) && ink(x, y)) {
                        stray += 1;
                    }
                }
                columns > 0 && stray * 2 > columns
            };
            if off_the_line(left, box_left) {
                left = box_left;
            }
            if off_the_line(box_right, right) {
                right = box_right;
            }
            if right - left < height * 6
                || f64::from(count) > f64::from((box_right - box_left) * height) * 0.65
            {
                continue;
            }
            if line
                .iter()
                .map(|&i| boxes[i].width())
                .fold(f32::NEG_INFINITY, f32::max)
                < height as f32 * 2.5
            {
                continue;
            }
            let (mut gap, mut max_gap) = (0, 0);
            for x in left..right {
                gap = if inked[x as usize] { 0 } else { gap + 1 };
                max_gap = max_gap.max(gap);
            }
            if max_gap > height {
                continue;
            }
            let (
                mut missing,
                mut missing_top,
                mut missing_bottom,
                mut missing_left,
                mut missing_right,
            ) = (0, bottom, top, right, left);
            for y in top..bottom {
                for x in left..right {
                    if !ink(x, y) || !colored(x, y) || line.iter().any(|&i| boxes[i].contains(x, y))
                    {
                        continue;
                    }
                    missing += 1;
                    missing_left = missing_left.min(x);
                    missing_right = missing_right.max(x + 1);
                    missing_top = missing_top.min(y);
                    missing_bottom = missing_bottom.max(y + 1);
                }
            }
            if missing < height
                || f64::from(missing_bottom - missing_top) < f64::from(height) * 0.3
                || f64::from(missing_right - missing_left) < f64::from(height) * 0.2
            {
                continue;
            }
            let margin = (height as f32 * 0.1).max(2.0);
            let bounds = Bounds {
                left: (left as f32 - margin).max(0.0),
                top: (ink_top as f32 - margin).max(0.0),
                right: (right as f32 + margin).min(width as f32),
                bottom: (ink_bottom as f32 + margin).min(image_height as f32),
            };
            if boxes
                .iter()
                .enumerate()
                .any(|(i, &b)| !line.contains(&i) && bounds.intersects(b))
            {
                continue;
            }
            repairs.push(Repair {
                owners: line,
                bounds,
            });
        }
    }
    repairs.sort_by_key(|r| r.owners.iter().copied().min());
    repairs
}
