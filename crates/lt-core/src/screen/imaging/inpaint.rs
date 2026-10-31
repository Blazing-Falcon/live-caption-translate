//! Fills a masked region from its surroundings, fast marching from the hole boundary inwards.

use super::{saturate, ImageBuffer, PixelType};
use std::{cmp::Ordering, collections::BinaryHeap};
/// Fast-marching states: pixels with a value, the advancing band, and the hole not yet reached.
const KNOWN: u8 = 0;
const BAND: u8 = 1;
const INSIDE: u8 = 2;
#[derive(PartialEq)]
struct Node {
    time: f32,
    order: usize,
    i: usize,
    j: usize,
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .time
            .total_cmp(&self.time)
            .then(other.order.cmp(&self.order))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
fn solve(p1: usize, p2: usize, states: &[u8], times: &[f32]) -> f32 {
    let a = f64::from(times[p1]);
    let b = f64::from(times[p2]);
    let minimum = a.min(b);
    let sol = if states[p1] != INSIDE {
        if states[p2] != INSIDE {
            if (a - b).abs() >= 1.0 {
                1.0 + minimum
            } else {
                (a + b + (2.0 - (a - b) * (a - b)).sqrt()) * 0.5
            }
        } else {
            1.0 + a
        }
    } else if states[p2] != INSIDE {
        1.0 + b
    } else {
        1.0 + minimum
    };
    sol as f32
}
pub fn navier_stokes(source: &ImageBuffer, mask: &ImageBuffer, radius: f64) -> ImageBuffer {
    assert_eq!(source.pixel_type, PixelType::U8C3);
    source.require_mask(mask);
    let target = source.isolated();
    if !source.is_empty() {
        run(&target, mask, radius);
    }
    target
}
fn run(target: &ImageBuffer, mask: &ImageBuffer, radius: f64) {
    let range = saturate::to_int(radius).clamp(1, 100);
    let erows = target.height + 2;
    let ecols = target.width + 2;
    let cells = erows * ecols;
    let mut states = vec![KNOWN; cells];
    let mut times = vec![1e6f32; cells];
    let mut heap = BinaryHeap::new();
    let mut order = 0;
    for i in 0..target.height {
        for j in 0..target.width {
            if mask.byte(i, j) != 0 {
                states[(i + 1) * ecols + j + 1] = INSIDE;
            }
        }
    }
    for i in 1..erows - 1 {
        for j in 1..ecols - 1 {
            let p = i * ecols + j;
            if states[p] != 0 {
                continue;
            }
            if states[p - ecols] != 0
                || states[p + ecols] != 0
                || states[p - 1] != 0
                || states[p + 1] != 0
            {
                heap.push(Node {
                    time: 0.0,
                    order,
                    i,
                    j,
                });
                order += 1;
                times[p] = 0.0;
            }
        }
    }
    let mut disc = Vec::new();
    for k in -range..=range {
        for l in -range..=range {
            if l * l + k * k <= range * range {
                let ry = k as f32;
                let rx = l as f32;
                let length = rx * rx + ry * ry;
                disc.push((k, l, rx, ry, length, 1.0 / (length * length + 1.0)));
            }
        }
    }
    while let Some(node) = heap.pop() {
        let (ii, jj) = (node.i, node.j);
        states[ii * ecols + jj] = KNOWN;
        for q in 0..4 {
            let (i, j) = match q {
                0 => (ii - 1, jj),
                1 => (ii, jj - 1),
                2 => (ii + 1, jj),
                _ => (ii, jj + 1),
            };
            if i == 0 || j == 0 || i > erows - 1 || j > ecols - 1 {
                continue;
            }
            let p = i * ecols + j;
            if states[p] != INSIDE {
                continue;
            }
            let mut a = solve((i - 1) * ecols + j, i * ecols + j - 1, &states, &times);
            let b = solve((i + 1) * ecols + j, i * ecols + j - 1, &states, &times);
            let mut c = solve((i - 1) * ecols + j, i * ecols + j + 1, &states, &times);
            let d = solve((i + 1) * ecols + j, i * ecols + j + 1, &states, &times);
            a = a.min(b);
            c = c.min(d);
            let dist = a.min(c);
            times[p] = dist;
            let mut intensity = [0f32; 3];
            let mut sum = [1e-20f32; 3];
            let interior = i as i32 - range >= 2
                && i as i32 + range <= erows as i32 - 3
                && j as i32 - range >= 2
                && j as i32 + range <= ecols as i32 - 3;
            for &(dk, dl, rx, ry, rlen, dst) in &disc {
                let k = i as i32 + dk;
                let l = j as i32 + dl;
                if !interior && !(k > 0 && l > 0 && k < erows as i32 - 1 && l < ecols as i32 - 1) {
                    continue;
                }
                let k = k as usize;
                let l = l as usize;
                let at = k * ecols + l;
                if states[at] == INSIDE {
                    continue;
                }
                let km = k - 1 + usize::from(k == 1);
                let kp = k - 1 - usize::from(k == erows - 2);
                let lm = l - 1 + usize::from(l == 1);
                let lp = l - 1 - usize::from(l == ecols - 2);
                let down = states[at + ecols] != INSIDE;
                let up = states[at - ecols] != INSIDE;
                let right = states[at + 1] != INSIDE;
                let left = states[at - 1] != INSIDE;
                for color in 0..3 {
                    let value = |y: usize, x: usize| i32::from(target.byte(y, x * 3 + color));
                    let mut gx = if down {
                        if up {
                            ((value(kp + 1, lm) - value(kp, lm)).abs()
                                + (value(kp, lm) - value(km - 1, lm)).abs())
                                as f32
                        } else {
                            (value(kp + 1, lm) - value(kp, lm)).abs() as f32 * 2.0
                        }
                    } else if up {
                        (value(kp, lm) - value(km - 1, lm)).abs() as f32 * 2.0
                    } else {
                        0.0
                    };
                    let gy = if right {
                        if left {
                            ((value(km, lp + 1) - value(km, lm)).abs()
                                + (value(km, lm) - value(km, lm - 1)).abs())
                                as f32
                        } else {
                            (value(km, lp + 1) - value(km, lm)).abs() as f32 * 2.0
                        }
                    } else if left {
                        (value(km, lm) - value(km, lm - 1)).abs() as f32 * 2.0
                    } else {
                        0.0
                    };
                    gx = -gx;
                    let mut dir = rx * gx + ry * gy;
                    if dir.abs() <= 0.01 {
                        dir = 0.000001;
                    } else {
                        dir = ((rx * gx + ry * gy) / (rlen * (gx * gx + gy * gy)).sqrt()).abs();
                    }
                    let w = dst * dir;
                    intensity[color] += w * value(k - 1, l - 1) as f32;
                    sum[color] += w;
                }
            }
            for color in 0..3 {
                target.set_byte(
                    i - 1,
                    (j - 1) * 3 + color,
                    saturate::to_byte(f64::from(intensity[color]) / f64::from(sum[color])),
                );
            }
            states[p] = BAND;
            heap.push(Node {
                time: dist,
                order,
                i,
                j,
            });
            order += 1;
        }
    }
}
