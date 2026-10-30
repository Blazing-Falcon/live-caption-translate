#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

const NEIGHBORS: [(i32, i32); 8] = [
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
];

fn neighbor(
    f: &[i32],
    w: usize,
    center: (i32, i32),
    previous: (i32, i32),
    clockwise: bool,
    offset: i32,
) -> Option<(i32, i32)> {
    let id = NEIGHBORS
        .iter()
        .position(|&(di, dj)| (center.0 + di, center.1 + dj) == previous)
        .unwrap() as i32;
    for k in 0..8 {
        let index = if clockwise {
            -k + id - offset + 16
        } else {
            k + id + offset + 16
        } % 8;
        let (di, dj) = NEIGHBORS[index as usize];
        let p = (center.0 + di, center.1 + dj);
        if f[p.0 as usize * w + p.1 as usize] != 0 {
            return Some(p);
        }
    }
    None
}

pub fn outlines(mut f: Vec<i32>, w: usize, h: usize) -> Vec<Vec<Point>> {
    for i in 1..h - 1 {
        f[i * w] = 0;
        f[i * w + w - 1] = 0;
    }
    for i in 0..w {
        f[i] = 0;
        f[w * h - 1 - i] = 0;
    }
    let mut id = 1;
    let mut result = Vec::new();
    for i in 1..h - 1 {
        for j in 1..w - 1 {
            let start = (i as i32, j as i32);
            let (hole, previous) = if f[i * w + j] == 1 && f[i * w + j - 1] == 0 {
                (false, (start.0, start.1 - 1))
            } else if f[i * w + j] >= 1 && f[i * w + j + 1] == 0 {
                (true, (start.0, start.1 + 1))
            } else {
                continue;
            };
            id += 1;
            let mut points = vec![Point::new(j as f32, i as f32)];
            if let Some(first) = neighbor(&f, w, start, previous, true, 0) {
                let mut previous = first;
                let mut current = start;
                loop {
                    let next = neighbor(&f, w, current, previous, false, 1).unwrap();
                    points.push(Point::new(next.1 as f32, next.0 as f32));
                    let index = current.0 as usize * w + current.1 as usize;
                    if f[index + 1] == 0 {
                        f[index] = -id;
                    } else if f[index] == 1 {
                        f[index] = id;
                    }
                    if next == start && current == first {
                        break;
                    }
                    previous = current;
                    current = next;
                }
            } else {
                f[i * w + j] = -id;
            }
            if !hole {
                result.push(simplify(&points, 1.0));
            }
        }
    }
    result
}

fn distance(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx * dx + dy * dy;
    let t = if len == 0.0 {
        -1.0
    } else {
        ((p.x - a.x) * dx + (p.y - a.y) * dy) / len
    };
    let (x, y) = if t < 0.0 {
        (a.x, a.y)
    } else if t > 1.0 {
        (b.x, b.y)
    } else {
        (a.x + t * dx, a.y + t * dy)
    };
    ((p.x - x) * (p.x - x) + (p.y - y) * (p.y - y)).sqrt()
}

fn simplify(points: &[Point], epsilon: f32) -> Vec<Point> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut maximum = 0.0;
    let mut index = 0;
    for i in 1..points.len() - 1 {
        let d = distance(points[i], points[0], points[points.len() - 1]);
        if d > maximum {
            maximum = d;
            index = i;
        }
    }
    if maximum > epsilon {
        let mut left = simplify(&points[..=index], epsilon);
        left.pop();
        left.extend(simplify(&points[index..], epsilon));
        left
    } else {
        vec![points[0], points[points.len() - 1]]
    }
}

fn ccw(a: Point, b: Point, c: Point) -> bool {
    (b.x - a.x) * (c.y - a.y) > (b.y - a.y) * (c.x - a.x)
}

fn hull(points: &[Point]) -> Vec<Point> {
    let mut sorted = Vec::new();
    for &p in points {
        if !sorted.contains(&p) {
            sorted.push(p);
        }
    }
    if sorted.len() < 3 {
        return sorted;
    }
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let origin = sorted[0];
    let mut groups: Vec<(f32, Point)> = Vec::new();
    for &p in &sorted[1..] {
        let angle = (p.y - origin.y).atan2(p.x - origin.x) % std::f32::consts::PI;
        if let Some((_, value)) = groups.iter_mut().find(|(a, _)| *a == angle) {
            let squared = |p: Point| {
                (p.x - origin.x) * (p.x - origin.x) + (p.y - origin.y) * (p.y - origin.y)
            };
            if squared(p) > squared(*value) {
                *value = p;
            }
        } else {
            groups.push((angle, p));
        }
    }
    groups.sort_by(|a, b| a.0.total_cmp(&b.0));
    if groups.len() < 2 {
        return vec![origin, groups[0].1];
    }
    let mut stack = vec![origin, groups[0].1, groups[1].1];
    for &(_, p) in &groups[2..] {
        while stack.len() > 1 && !ccw(stack[stack.len() - 2], stack[stack.len() - 1], p) {
            stack.pop();
        }
        stack.push(p);
    }
    stack.reverse();
    stack
}

pub fn minimum_rectangle(points: &[Point]) -> Vec<Point> {
    let polygon = hull(points);
    if polygon.len() <= 2 {
        return polygon;
    }
    let mut best = [0.0; 8];
    let mut minimum = f32::INFINITY;
    let (mut qx, mut qy, mut r0x, mut r0y, mut r1x, mut r1y) =
        (f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN);
    for k in 0..polygon.len() {
        let a = polygon[k];
        let b = polygon[(k + 1) % polygon.len()];
        let (vx, vy) = (b.x - a.x, b.y - a.y);
        let r = 1.0 / (vx * vx + vy * vy);
        let (mut tmin, mut tmax, mut smax, mut far) = (1.0, 0.0, 0.0, None);
        for (j, p) in polygon.iter().enumerate() {
            let t = ((p.x - a.x) * vx + (p.y - a.y) * vy) * r;
            let (px, py) = (t * vx + a.x, t * vy + a.y);
            let (ux, uy) = (px - p.x, py - p.y);
            let s = ux * ux + uy * uy;
            if t < tmin {
                tmin = t;
                r0x = px;
                r0y = py;
            }
            if t > tmax {
                tmax = t;
                r1x = px;
                r1y = py;
            }
            if s > smax {
                smax = s;
                qx = px;
                qy = py;
                far = Some(j);
            }
        }
        if let Some(j) = far {
            let p = polygon[j];
            let (dx, dy) = (p.x - qx, p.y - qy);
            let area = ((r1x - r0x) * (r1x - r0x) + (r1y - r0y) * (r1y - r0y)) * smax;
            if area < minimum {
                minimum = area;
                best = [r0x, r0y, r1x, r1y, r1x + dx, r1y + dy, r0x + dx, r0y + dy];
            }
        }
    }
    vec![
        Point::new(best[4], best[5]),
        Point::new(best[6], best[7]),
        Point::new(best[2], best[3]),
        Point::new(best[0], best[1]),
    ]
}

pub fn size(points: &[Point]) -> (f32, f32) {
    if points.len() < 4 {
        return (f32::NAN, f32::NAN);
    }
    let length =
        |a: Point, b: Point| ((a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y)).sqrt();
    (length(points[2], points[3]), length(points[2], points[0]))
}

pub fn mini_box(points: &[Point]) -> Option<([Point; 4], f32)> {
    let mut points = minimum_rectangle(points);
    if points.len() < 4 {
        return None;
    }
    let (width, height) = size(&points);
    points.sort_by(|a, b| a.x.total_cmp(&b.x));
    let (a, d) = if points[1].y > points[0].y {
        (0, 1)
    } else {
        (1, 0)
    };
    let (b, c) = if points[3].y > points[2].y {
        (2, 3)
    } else {
        (3, 2)
    };
    Some((
        [points[a], points[b], points[c], points[d]],
        if width.is_nan() || height.is_nan() {
            f32::NAN
        } else {
            width.min(height)
        },
    ))
}

pub fn unclip(box_points: &[Point; 4], ratio: f32) -> Option<Vec<Point>> {
    let rectangle = minimum_rectangle(box_points);
    let (width, height) = size(&rectangle);
    if width.is_nan() || height.is_nan() || (height < 1.001 && width < 1.001) {
        return None;
    }
    let mut area = 0.0_f32;
    let mut length = 0.0_f64;
    for i in 0..4 {
        let a = box_points[i];
        let b = box_points[(i + 1) % 4];
        area += (b.x - a.x) * (b.y + a.y) / 2.0;
        let (dx, dy) = (
            f64::from(b.x) - f64::from(a.x),
            f64::from(b.y) - f64::from(a.y),
        );
        length += (dx * dx + dy * dy).sqrt();
    }
    let delta = f64::from(area.abs() * ratio) / length;
    let points = box_points.map(|p| (f64::from(p.x).round(), f64::from(p.y).round()));
    let mut normals = [(0.0, 0.0); 4];
    for i in 0..4 {
        let a = points[i];
        let b = points[(i + 1) % 4];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let inverse = 1.0 / (dx * dx + dy * dy).sqrt();
        normals[i] = (dy * inverse, -dx * inverse);
    }
    const ARC_CONST: f64 = 0.002;
    let arc_tolerance = delta.abs() * ARC_CONST;
    let steps_per_circle = std::f64::consts::PI / (1.0 - arc_tolerance / delta.abs()).acos();
    let step_angle = 2.0 * std::f64::consts::PI / steps_per_circle;
    let (sin, cos) = (step_angle.sin(), step_angle.cos());
    let steps_per_rad = steps_per_circle / (2.0 * std::f64::consts::PI);
    let mut output = Vec::new();
    for j in 0..4 {
        let k = (j + 3) % 4;
        let previous = normals[k];
        let next = normals[j];
        let sin_a = (previous.0 * next.1 - previous.1 * next.0).clamp(-1.0, 1.0);
        let cos_a = next.0 * previous.0 + next.1 * previous.1;
        let angle = sin_a.atan2(cos_a);
        let mut offset = (previous.0 * delta, previous.1 * delta);
        let mut add = |v: (f64, f64)| {
            output.push(Point::new(
                (points[j].0 + v.0).round() as f32,
                (points[j].1 + v.1).round() as f32,
            ))
        };
        add(offset);
        let steps = (steps_per_rad * angle.abs()).ceil() as usize;
        for _ in 1..steps {
            offset = (
                offset.0 * cos - sin * offset.1,
                offset.0 * sin + offset.1 * cos,
            );
            add(offset);
        }
        add((next.0 * delta, next.1 * delta));
    }
    Some(output)
}
