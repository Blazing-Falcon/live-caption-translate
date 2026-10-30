use super::ImageView;

#[derive(Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl Raster {
    pub fn view(&self) -> ImageView<'_> {
        ImageView {
            width: self.width,
            height: self.height,
            stride: self.width * 4,
            bgra: &self.bgra,
        }
    }

    pub fn resize(source: &ImageView<'_>, width: u32, height: u32) -> Self {
        if width == source.width && height == source.height {
            return Self {
                width,
                height,
                bgra: (0..height)
                    .flat_map(|y| source.row(y).iter().copied())
                    .collect(),
            };
        }
        Self::resample(source, width, height)
    }

    pub fn resample(source: &ImageView<'_>, width: u32, height: u32) -> Self {
        let mut output = Self {
            width,
            height,
            bgra: vec![0; width as usize * height as usize * 4],
        };
        let scale_x = source.width as f32 / width as f32;
        let scale_y = source.height as f32 / height as f32;
        for y in 0..height {
            for x in 0..width {
                let pixel = cubic(
                    source,
                    (x as f32 + 0.5) * scale_x - 0.5,
                    (y as f32 + 0.5) * scale_y - 0.5,
                );
                output.bgra[((y * width + x) * 4) as usize..][..4].copy_from_slice(&pixel);
            }
        }
        output
    }

    pub fn clockwise(&self) -> Self {
        let filtered = Self::resample(&self.view(), self.width, self.height);
        let mut output = Self {
            width: self.height,
            height: self.width,
            bgra: vec![0; self.bgra.len()],
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let destination = ((x * self.height + self.height - 1 - y) * 4) as usize;
                let source = ((y * self.width + x) * 4) as usize;
                output.bgra[destination..][..4].copy_from_slice(&filtered.bgra[source..][..4]);
            }
        }
        output
    }
}

fn kernel(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        ((7.0 / 6.0 * x - 2.0) * x) * x + 8.0 / 9.0
    } else if x < 2.0 {
        ((-7.0 / 18.0 * x + 2.0) * x - 10.0 / 3.0) * x + 16.0 / 9.0
    } else {
        0.0
    }
}

pub fn cubic(source: &ImageView<'_>, x: f32, y: f32) -> [u8; 4] {
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let mut result = [0.0_f32; 4];
    for dy in -1..=2 {
        let row = source.row((iy + dy).clamp(0, source.height as i32 - 1) as u32);
        let wy = kernel(y - (iy + dy) as f32);
        for dx in -1..=2 {
            let weight = wy * kernel(x - (ix + dx) as f32);
            let offset = (ix + dx).clamp(0, source.width as i32 - 1) as usize * 4;
            for channel in 0..4 {
                result[channel] += f32::from(row[offset + channel]) * weight;
            }
        }
    }
    let alpha = result[3].clamp(0.0, 255.0);
    [
        result[0].clamp(0.0, alpha).round() as u8,
        result[1].clamp(0.0, alpha).round() as u8,
        result[2].clamp(0.0, alpha).round() as u8,
        alpha.round() as u8,
    ]
}

pub fn crop(source: &ImageView<'_>, points: &[(i32, i32); 4]) -> Raster {
    let left = points.iter().map(|p| p.0).min().unwrap();
    let right = points.iter().map(|p| p.0).max().unwrap();
    let top = points.iter().map(|p| p.1).min().unwrap();
    let bottom = points.iter().map(|p| p.1).max().unwrap();
    let width = (right - left) as u32;
    let height = (bottom - top) as u32;
    let mut subset = Raster {
        width,
        height,
        bgra: vec![0; width as usize * height as usize * 4],
    };
    for y in 0..height {
        let row = source.row(top as u32 + y);
        subset.bgra[(y * width * 4) as usize..][..width as usize * 4]
            .copy_from_slice(&row[left as usize * 4..right as usize * 4]);
    }
    let p = points.map(|(x, y)| (x - left, y - top));
    let length = |a: (i32, i32), b: (i32, i32)| {
        f64::from((a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1)).sqrt() as u32
    };
    let target_width = length(p[0], p[1]);
    let target_height = length(p[0], p[3]);
    let x1 = p[0].0 as f32;
    let y1 = p[0].1 as f32;
    let x2 = p[1].0 as f32;
    let y2 = p[1].1 as f32;
    let x3 = p[2].0 as f32;
    let y3 = p[2].1 as f32;
    let x4 = p[3].0 as f32;
    let y4 = p[3].1 as f32;
    let w = target_width as f32;
    let h = target_height as f32;
    let scale_x: f32 = (y1 * x2 * x4 - x1 * y2 * x4 + x1 * y3 * x4 - x2 * y3 * x4 - y1 * x2 * x3
        + x1 * y2 * x3
        - x1 * y4 * x3
        + x2 * y4 * x3)
        / (x2 * y3 * w + y2 * x4 * w - y3 * x4 * w - x2 * y4 * w - y2 * w * x3 + y4 * w * x3);
    let skew_x: f32 =
        (-x1 * x2 * y3 - y1 * x2 * x4 + x2 * y3 * x4 + x1 * x2 * y4 + x1 * y2 * x3 + y1 * x4 * x3
            - y2 * x4 * x3
            - x1 * y4 * x3)
            / (x2 * y3 * h + y2 * x4 * h - y3 * x4 * h - x2 * y4 * h - y2 * h * x3 + y4 * h * x3);
    let trans_x: f32 = x1;
    let skew_y: f32 = (-y1 * x2 * y3 + x1 * y2 * y3 + y1 * y3 * x4 - y2 * y3 * x4 + y1 * x2 * y4
        - x1 * y2 * y4
        - y1 * y4 * x3
        + y2 * y4 * x3)
        / (x2 * y3 * w + y2 * x4 * w - y3 * x4 * w - x2 * y4 * w - y2 * w * x3 + y4 * w * x3);
    let scale_y: f32 = (-y1 * x2 * y3 - y1 * y2 * x4 + y1 * y3 * x4 + x1 * y2 * y4 - x1 * y3 * y4
        + x2 * y3 * y4
        + y1 * y2 * x3
        - y2 * y4 * x3)
        / (x2 * y3 * h + y2 * x4 * h - y3 * x4 * h - x2 * y4 * h - y2 * h * x3 + y4 * h * x3);
    let trans_y: f32 = y1;
    let persp0: f32 = (x1 * y3 - x2 * y3 + y1 * x4 - y2 * x4 - x1 * y4 + x2 * y4 - y1 * x3
        + y2 * x3)
        / (x2 * y3 * w + y2 * x4 * w - y3 * x4 * w - x2 * y4 * w - y2 * w * x3 + y4 * w * x3);
    let persp1: f32 = (-y1 * x2 + x1 * y2 - x1 * y3 - y2 * x4 + y3 * x4 + x2 * y4 + y1 * x3
        - y4 * x3)
        / (x2 * y3 * h + y2 * x4 * h - y3 * x4 * h - x2 * y4 * h - y2 * h * x3 + y4 * h * x3);
    let persp2: f32 = 1.0;
    let identity = scale_x == 1.0
        && skew_x == 0.0
        && trans_x == 0.0
        && skew_y == 0.0
        && scale_y == 1.0
        && trans_y == 0.0
        && persp0 == 0.0
        && persp1 == 0.0;
    let mut part = if identity {
        subset
    } else {
        let mut output = Raster {
            width: target_width,
            height: target_height,
            bgra: vec![0; target_width as usize * target_height as usize * 4],
        };
        for y in 0..target_height {
            for x in 0..target_width {
                let dx = x as f32 + 0.5;
                let dy = y as f32 + 0.5;
                let denominator = persp0 * dx + persp1 * dy + persp2;
                let sx = (scale_x * dx + skew_x * dy + trans_x) / denominator;
                let sy = (skew_y * dx + scale_y * dy + trans_y) / denominator;
                if sx < 0.0 || sy < 0.0 || sx >= width as f32 || sy >= height as f32 {
                    continue;
                }
                let pixel = cubic(&subset.view(), sx - 0.5, sy - 0.5);
                output.bgra[((y * target_width + x) * 4) as usize..][..4].copy_from_slice(&pixel);
            }
        }
        output
    };
    if part.height as f64 >= f64::from(part.width) * 1.5 {
        part = part.clockwise();
    }
    part
}
