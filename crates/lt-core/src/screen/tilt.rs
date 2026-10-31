use super::{
    geom::{max, min, LineGeometry, PointF, RectF},
    line::{layout_glyph_height_for, Line, SizeF, TiltedLine},
};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, f64::consts::PI};

const MIN_DEGREES: f64 = 3.0;
const MAX_DEGREES: f64 = LineGeometry::TILTED_TO_DEGREES;
const MIN_LENGTH_TO_THICKNESS: f64 = 3.0;
const MAX_ANGLE_SPREAD: f64 = 5.0;
const LOCAL_ANGLE_LINES: usize = 3;
const LONE_MIN_DEGREES: f64 = 6.0;
const LONE_MIN_LENGTH_TO_THICKNESS: f64 = 5.0;
const ERASE_PADDING: f64 = 0.18;
const MARGIN_INSET: f64 = 0.12;
pub const OVERLAP_TOLERANCE: f64 = 0.03;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TiltedText {
    pub degrees: f64,
    pub sheared: bool,
    pub centre: PointF,
    #[serde(rename = "box")]
    pub box_rect: RectF,
    #[serde(alias = "lineQuads")]
    pub line_quads: Vec<Vec<PointF>>,
    #[serde(default)]
    pub column: bool,
}

fn centre(box_rect: RectF) -> PointF {
    PointF::new(
        box_rect.x + box_rect.width / 2.0,
        box_rect.y + box_rect.height / 2.0,
    )
}
fn distance(a: PointF, b: PointF) -> f64 {
    ((b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y)).sqrt()
}
fn average(points: &[PointF]) -> PointF {
    PointF::new(
        points.iter().map(|p| p.x).sum::<f64>() / points.len() as f64,
        points.iter().map(|p| p.y).sum::<f64>() / points.len() as f64,
    )
}

impl TiltedText {
    pub fn to_image(&self, level: PointF) -> PointF {
        let dx = level.x - self.centre.x;
        let dy = level.y - self.centre.y;
        let radians = self.degrees * PI / 180.0;
        if self.sheared {
            return PointF::new(level.x, level.y + dx * radians.tan());
        }
        let (cos, sin) = (radians.cos(), radians.sin());
        PointF::new(
            self.centre.x + dx * cos - dy * sin,
            self.centre.y + dx * sin + dy * cos,
        )
    }

    pub fn to_level(&self, image: PointF) -> PointF {
        let dx = image.x - self.centre.x;
        let dy = image.y - self.centre.y;
        let radians = self.degrees * PI / 180.0;
        if self.sheared {
            return PointF::new(image.x, image.y - dx * radians.tan());
        }
        let (cos, sin) = (radians.cos(), radians.sin());
        PointF::new(
            self.centre.x + dx * cos + dy * sin,
            self.centre.y - dx * sin + dy * cos,
        )
    }

    pub fn corners(&self, level: RectF) -> [PointF; 4] {
        level.corners().map(|p| self.to_image(p))
    }
    pub fn outline(&self) -> [PointF; 4] {
        self.corners(self.box_rect)
    }

    pub fn erase_quads(&self) -> Vec<Vec<PointF>> {
        self.line_quads
            .iter()
            .map(|quad| {
                let padded = padded(quad);
                if self.column {
                    padded
                } else {
                    self.clip_at_margin(&padded, quad)
                }
            })
            .collect()
    }

    pub fn sample_area(&self) -> RectF {
        let middle = self.to_image(centre(self.box_rect));
        let height = self.box_rect.height / 2.0;
        let level = self.box_rect.inflate(0.5, 0.5);
        let mut width = self.box_rect.width;
        while width > 1.0 {
            let area = RectF::new(
                middle.x - width / 2.0,
                middle.y - height / 2.0,
                width,
                height,
            );
            if area
                .corners()
                .into_iter()
                .all(|p| level.contains(self.to_level(p)))
            {
                return area;
            }
            width *= 0.9;
        }
        RectF::new(middle.x - 0.5, middle.y - height / 2.0, 1.0, height)
    }

    pub fn offset(&self, dx: f64, dy: f64) -> Self {
        Self {
            centre: PointF::new(self.centre.x + dx, self.centre.y + dy),
            box_rect: self.box_rect.offset(dx, dy),
            line_quads: self
                .line_quads
                .iter()
                .map(|quad| {
                    quad.iter()
                        .map(|p| PointF::new(p.x + dx, p.y + dy))
                        .collect()
                })
                .collect(),
            ..self.clone()
        }
    }

    pub fn from_lines(lines: &[TiltedLine]) -> Option<Self> {
        let first = lines.first()?;
        if lines
            .iter()
            .any(|line| line.cluster != first.cluster || line.sheared != first.sheared)
        {
            return None;
        }
        let weight: f64 = lines.iter().map(|line| line.weight).sum();
        let degrees = if weight > 0.0 {
            lines
                .iter()
                .map(|line| line.degrees * line.weight)
                .sum::<f64>()
                / weight
        } else {
            lines.iter().map(|line| line.degrees).sum::<f64>() / lines.len() as f64
        };
        let mut frame = Self {
            degrees,
            sheared: first.sheared,
            centre: average(&lines.iter().map(|line| line.centre).collect::<Vec<_>>()),
            box_rect: RectF::EMPTY,
            line_quads: lines.iter().map(|line| line.quad.clone()).collect(),
            column: false,
        };
        let boxes: Vec<_> = lines
            .iter()
            .map(|line| {
                let middle = frame.to_level(line.centre);
                RectF::new(
                    middle.x - line.level.width / 2.0,
                    middle.y - line.level.height / 2.0,
                    line.level.width,
                    line.level.height,
                )
            })
            .collect();
        let mut lefts: Vec<_> = boxes.iter().map(|b| b.x).collect();
        lefts.sort_by(f64::total_cmp);
        let margin = lefts[lefts.len() / 2];
        let right = boxes.iter().map(|b| b.right()).reduce(max).unwrap();
        let top = boxes.iter().map(|b| b.y).reduce(min).unwrap();
        let bottom = boxes.iter().map(|b| b.bottom()).reduce(max).unwrap();
        if right <= margin {
            return None;
        }
        frame.box_rect = RectF::new(margin, top, right - margin, bottom - top);
        Some(frame)
    }

    pub fn from_columns(degrees: f64, quads: &[Vec<PointF>]) -> Option<Self> {
        if quads.is_empty() || quads.iter().any(|quad| quad.len() != 4) {
            return None;
        }
        let mut frame = Self {
            degrees,
            sheared: false,
            centre: average(&quads.iter().map(|quad| average(quad)).collect::<Vec<_>>()),
            box_rect: RectF::EMPTY,
            line_quads: quads.to_vec(),
            column: true,
        };
        frame.box_rect = RectF::enclosing(
            &quads
                .iter()
                .flatten()
                .map(|p| frame.to_level(*p))
                .collect::<Vec<_>>(),
        );
        (!frame.box_rect.is_empty() && frame.box_rect.width > 0.0 && frame.box_rect.height > 0.0)
            .then_some(frame)
    }

    pub fn inside(polygon: &[PointF], x: f64, y: f64) -> bool {
        Self::depth(polygon, x, y) >= 0.0
    }
    pub fn depth(polygon: &[PointF], x: f64, y: f64) -> f64 {
        let signed = signed_twice_area(polygon);
        if signed == 0.0 {
            return f64::NEG_INFINITY;
        }
        let turn = signed.signum();
        let mut depth = f64::MAX;
        for i in 0..polygon.len() {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            let length = distance(a, b);
            if length == 0.0 {
                continue;
            }
            let cross = (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x);
            depth = min(depth, turn * cross / length);
        }
        depth
    }

    fn clip_at_margin(&self, polygon: &[PointF], quad: &[PointF]) -> Vec<PointF> {
        if self.box_rect.is_empty() || quad.len() != 4 {
            return polygon.to_vec();
        }
        let edge = self.box_rect.x + distance(quad[0], quad[3]) * MARGIN_INSET;
        let level: Vec<_> = polygon.iter().map(|p| self.to_level(*p)).collect();
        let mut kept = Vec::new();
        for i in 0..level.len() {
            let (from, to) = (level[i], level[(i + 1) % level.len()]);
            let (from_in, to_in) = (from.x >= edge, to.x >= edge);
            if from_in {
                kept.push(from);
            }
            if from_in != to_in {
                let t = (edge - from.x) / (to.x - from.x);
                kept.push(PointF::new(edge, from.y + (to.y - from.y) * t));
            }
        }
        if kept.len() >= 3 {
            kept.into_iter().map(|p| self.to_image(p)).collect()
        } else {
            polygon.to_vec()
        }
    }
}

fn padded(quad: &[PointF]) -> Vec<PointF> {
    if quad.len() != 4 {
        return quad.to_vec();
    }
    let middle = average(quad);
    let along = PointF::new((quad[1].x - quad[0].x) / 2.0, (quad[1].y - quad[0].y) / 2.0);
    let across = PointF::new((quad[3].x - quad[0].x) / 2.0, (quad[3].y - quad[0].y) / 2.0);
    let length = distance(PointF::new(0.0, 0.0), along);
    let thickness = distance(PointF::new(0.0, 0.0), across) * 2.0;
    if length <= 0.0 || thickness <= 0.0 {
        return quad.to_vec();
    }
    let scale = (length + thickness * ERASE_PADDING) / length;
    let reach = PointF::new(along.x * scale, along.y * scale);
    let depth = PointF::new(
        across.x * (1.0 + ERASE_PADDING),
        across.y * (1.0 + ERASE_PADDING),
    );
    vec![
        PointF::new(middle.x - reach.x - depth.x, middle.y - reach.y - depth.y),
        PointF::new(middle.x + reach.x - depth.x, middle.y + reach.y - depth.y),
        PointF::new(middle.x + reach.x + depth.x, middle.y + reach.y + depth.y),
        PointF::new(middle.x - reach.x + depth.x, middle.y - reach.y + depth.y),
    ]
}

fn reliable(block: &Line) -> bool {
    block.line_geometry.is_some_and(|line| {
        line.length >= line.thickness * MIN_LENGTH_TO_THICKNESS
            && (MIN_DEGREES..=MAX_DEGREES).contains(&line.angle_degrees.abs())
    })
}
fn lone(block: &Line) -> bool {
    block.line_geometry.is_some_and(|line| {
        line.length >= line.thickness * LONE_MIN_LENGTH_TO_THICKNESS
            && (LONE_MIN_DEGREES..=MAX_DEGREES).contains(&line.angle_degrees.abs())
    })
}
fn corners(block: &Line) -> [PointF; 4] {
    let box_rect = block.layout_bounds;
    let Some(line) = block.line_geometry else {
        let p = box_rect.corners();
        return [p[0], p[1], p[3], p[2]];
    };
    let c = centre(box_rect);
    let radians = line.angle_degrees * PI / 180.0;
    let along = PointF::new(
        radians.cos() * (line.length / 2.0),
        radians.sin() * (line.length / 2.0),
    );
    let across = PointF::new(
        -radians.sin() * (line.thickness / 2.0),
        radians.cos() * (line.thickness / 2.0),
    );
    [
        PointF::new(c.x + along.x + across.x, c.y + along.y + across.y),
        PointF::new(c.x + along.x - across.x, c.y + along.y - across.y),
        PointF::new(c.x - along.x + across.x, c.y - along.y + across.y),
        PointF::new(c.x - along.x - across.x, c.y - along.y - across.y),
    ]
}
fn outline(block: &Line) -> Vec<PointF> {
    if block.line_geometry.is_none() {
        return block.layout_bounds.corners().to_vec();
    }
    let p = corners(block);
    vec![p[3], p[1], p[0], p[2]]
}
fn rotated_box(block: &Line, trusted: bool, degrees: f64, pivot: PointF) -> RectF {
    let radians = degrees * PI / 180.0;
    let (cos, sin) = (radians.cos(), radians.sin());
    let turned = corners(block).map(|p| {
        let (dx, dy) = (p.x - pivot.x, p.y - pivot.y);
        PointF::new(pivot.x + dx * cos - dy * sin, pivot.y + dx * sin + dy * cos)
    });
    let box_rect = RectF::enclosing(&turned);
    if let Some(line) = block.line_geometry.filter(|_| trusted) {
        let c = centre(box_rect);
        RectF::new(
            c.x - line.length / 2.0,
            c.y - line.thickness / 2.0,
            line.length,
            line.thickness,
        )
    } else {
        box_rect
    }
}
fn sheared_box(block: &Line, degrees: f64, pivot: PointF) -> RectF {
    let slope = (degrees * PI / 180.0).tan();
    RectF::enclosing(&corners(block).map(|p| PointF::new(p.x, p.y - (p.x - pivot.x) * slope)))
}
fn left_misalignment(indices: &[usize], place: impl Fn(usize) -> RectF) -> f64 {
    let mut boxes: Vec<_> = indices.iter().map(|i| place(*i)).collect();
    boxes.sort_by(|a, b| (a.y + a.height / 2.0).total_cmp(&(b.y + b.height / 2.0)));
    let mut offsets: Vec<_> = boxes
        .windows(2)
        .map(|p| (p[0].x - p[1].x).abs() / ((p[0].height + p[1].height) / 2.0))
        .collect();
    offsets.sort_by(f64::total_cmp);
    if offsets.is_empty() {
        0.0
    } else {
        offsets[offsets.len() / 2]
    }
}
fn line_pitch(boxes: &[RectF]) -> Option<f64> {
    let mut centres: Vec<_> = boxes.iter().map(|b| b.y + b.height / 2.0).collect();
    centres.sort_by(f64::total_cmp);
    let mut pitches: Vec<_> = centres
        .windows(2)
        .map(|p| p[1] - p[0])
        .filter(|p| *p > 2.0)
        .collect();
    pitches.sort_by(f64::total_cmp);
    (!pitches.is_empty()).then(|| pitches[pitches.len() / 4])
}
fn find(parent: &mut [usize], i: usize) -> usize {
    if parent[i] != i {
        parent[i] = find(parent, parent[i]);
    }
    parent[i]
}

pub fn straighten(blocks: &[Line]) -> Cow<'_, [Line]> {
    let count = blocks.len();
    if count == 0 {
        return Cow::Borrowed(blocks);
    }
    let trusted: Vec<_> = blocks.iter().map(reliable).collect();
    let mut parent: Vec<_> = (0..count).collect();
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    if trusted.iter().filter(|b| **b).count() >= 2 {
        for i in 0..count {
            for j in i + 1..count {
                if !trusted[i] || !trusted[j] {
                    continue;
                }
                if (blocks[i].line_geometry.unwrap().angle_degrees
                    - blocks[j].line_geometry.unwrap().angle_degrees)
                    .abs()
                    > MAX_ANGLE_SPREAD
                {
                    continue;
                }
                if !blocks[i]
                    .layout_bounds
                    .intersects_with(blocks[j].layout_bounds)
                {
                    continue;
                }
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a] = b;
            }
        }
        let mut roots = Vec::new();
        for (i, is_trusted) in trusted.iter().enumerate() {
            if !is_trusted {
                continue;
            }
            let root = find(&mut parent, i);
            let index = if let Some(index) = roots.iter().position(|r| *r == root) {
                index
            } else {
                roots.push(root);
                clusters.push(Vec::new());
                roots.len() - 1
            };
            clusters[index].push(i);
        }
        clusters.retain(|c| c.len() >= 2);
    }
    let mut member = vec![None; count];
    for (c, indices) in clusters.iter().enumerate() {
        for i in indices {
            member[*i] = Some(c);
        }
    }
    for i in 0..count {
        if member[i].is_some() {
            continue;
        }
        let c = centre(blocks[i].layout_bounds);
        for (cluster, indices) in clusters.iter().enumerate() {
            if indices.iter().any(|k| blocks[*k].layout_bounds.contains(c)) {
                member[i] = Some(cluster);
                break;
            }
        }
    }
    let mut result = Cow::Borrowed(blocks);
    for (c, indices) in clusters.iter().enumerate() {
        let all: Vec<_> = (0..count).filter(|i| member[*i] == Some(c)).collect();
        let pivot = average(
            &all.iter()
                .map(|i| centre(blocks[*i].layout_bounds))
                .collect::<Vec<_>>(),
        );
        let mut angles = vec![0.0; count];
        for i in &all {
            let middle = centre(blocks[*i].layout_bounds);
            let mut nearest = indices.clone();
            nearest.sort_by(|a, b| {
                distance(centre(blocks[*a].layout_bounds), middle)
                    .total_cmp(&distance(centre(blocks[*b].layout_bounds), middle))
            });
            nearest.truncate(LOCAL_ANGLE_LINES);
            angles[*i] = nearest
                .iter()
                .map(|j| {
                    let line = blocks[*j].line_geometry.unwrap();
                    line.angle_degrees * line.length
                })
                .sum::<f64>()
                / nearest
                    .iter()
                    .map(|j| blocks[*j].line_geometry.unwrap().length)
                    .sum::<f64>();
        }
        let rotated = |i: usize| rotated_box(&blocks[i], trusted[i], -angles[i], pivot);
        let sheared_place = |i: usize| sheared_box(&blocks[i], angles[i], pivot);
        let sheared =
            left_misalignment(indices, sheared_place) < left_misalignment(indices, rotated);
        let boxes: Vec<_> = all
            .iter()
            .map(|i| {
                if sheared {
                    sheared_place(*i)
                } else {
                    rotated(*i)
                }
            })
            .collect();
        let cap = line_pitch(
            &indices
                .iter()
                .map(|i| {
                    if sheared {
                        sheared_place(*i)
                    } else {
                        rotated(*i)
                    }
                })
                .collect::<Vec<_>>(),
        );
        for (i, mut box_rect) in all.into_iter().zip(boxes) {
            if let Some(height) = cap.filter(|height| box_rect.height > *height) {
                box_rect = RectF::new(
                    box_rect.x,
                    box_rect.y + (box_rect.height - height) / 2.0,
                    box_rect.width,
                    height,
                );
            }
            let block = &blocks[i];
            let level = if sheared {
                block.line_geometry.map(|line| SizeF {
                    width: line.length * (line.angle_degrees * PI / 180.0).cos(),
                    height: box_rect.height,
                })
            } else {
                None
            }
            .unwrap_or(SizeF {
                width: box_rect.width,
                height: box_rect.height,
            });
            let output = &mut result.to_mut()[i];
            output.layout_bounds = box_rect;
            output.layout_glyph_height =
                layout_glyph_height_for(block.layout_script, box_rect, &block.text);
            output.upright_layout_bounds =
                block.upright_layout_bounds.or(Some(block.layout_bounds));
            output.tilted_lines = Some(vec![TiltedLine {
                cluster: c as i32,
                sheared,
                degrees: angles[i],
                weight: if trusted[i] {
                    block.line_geometry.unwrap().length
                } else {
                    0.0
                },
                centre: centre(block.layout_bounds),
                level,
                quad: outline(block),
            }]);
        }
    }
    let mut next_cluster = clusters.len() as i32;
    for (i, block) in blocks.iter().enumerate() {
        if member[i].is_some() || !lone(block) {
            continue;
        }
        let line = block.line_geometry.unwrap();
        result.to_mut()[i].tilted_lines = Some(vec![TiltedLine {
            cluster: next_cluster,
            sheared: false,
            degrees: line.angle_degrees,
            weight: line.length,
            centre: centre(block.layout_bounds),
            level: SizeF {
                width: line.length,
                height: line.thickness,
            },
            quad: outline(block),
        }]);
        next_cluster += 1;
    }
    result
}

pub fn restore(groups: &[Line]) -> Cow<'_, [Line]> {
    let mut result = Cow::Borrowed(groups);
    for (i, group) in groups.iter().enumerate() {
        if group.upright_layout_bounds.is_none() && group.tilted_lines.is_none() {
            continue;
        }
        let output = &mut result.to_mut()[i];
        output.layout_bounds = group.upright_layout_bounds.unwrap_or(group.layout_bounds);
        output.upright_layout_bounds = None;
        output.tilted_lines = None;
        output.tilt = group
            .tilted_lines
            .as_deref()
            .and_then(TiltedText::from_lines);
    }
    result
}

fn signed_twice_area(polygon: &[PointF]) -> f64 {
    let mut area = 0.0;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        area += a.x * b.y - b.x * a.y;
    }
    area
}
pub fn area(polygon: &[PointF]) -> f64 {
    signed_twice_area(polygon).abs() / 2.0
}
pub fn intersection(subject: &[PointF], clip: &[PointF]) -> Vec<PointF> {
    let turn = signed_twice_area(clip);
    if turn == 0.0 {
        return Vec::new();
    }
    let turn = turn.signum();
    let mut output = subject.to_vec();
    for edge in 0..clip.len() {
        if output.is_empty() {
            break;
        }
        let (a, b) = (clip[edge], clip[(edge + 1) % clip.len()]);
        let side = |p: PointF| turn * ((b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x));
        let input = output;
        output = Vec::new();
        for i in 0..input.len() {
            let (from, to) = (input[i], input[(i + 1) % input.len()]);
            let (side_from, side_to) = (side(from), side(to));
            if side_from >= 0.0 {
                output.push(from);
            }
            if (side_from >= 0.0) != (side_to >= 0.0) {
                let t = side_from / (side_from - side_to);
                output.push(PointF::new(
                    from.x + (to.x - from.x) * t,
                    from.y + (to.y - from.y) * t,
                ));
            }
        }
    }
    output
}
pub fn level(groups: &[Line]) -> Cow<'_, [Line]> {
    if !groups.iter().any(|group| group.tilt.is_some()) {
        return Cow::Borrowed(groups);
    }
    let mut result = Cow::Borrowed(groups);
    let mut regions: Vec<_> = groups
        .iter()
        .map(|g| {
            g.tilt
                .as_ref()
                .map_or_else(|| g.bounds.corners(), TiltedText::outline)
                .to_vec()
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..groups.len() {
            if result[i].tilt.is_none() {
                continue;
            }
            let upright = result[i].bounds.corners();
            let turned = &regions[i];
            let turned_area = area(turned);
            let collides = regions.iter().enumerate().any(|(j, other)| {
                if j == i {
                    return false;
                }
                let smaller = min(turned_area, area(other));
                if smaller <= 0.0 {
                    return false;
                }
                let excess =
                    area(&intersection(turned, other)) - area(&intersection(&upright, other));
                excess > smaller * OVERLAP_TOLERANCE
            });
            if collides {
                result.to_mut()[i].tilt = None;
                regions[i] = upright.to_vec();
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    result
}
