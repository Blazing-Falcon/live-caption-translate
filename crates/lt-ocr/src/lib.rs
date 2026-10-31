//! PP-OCRv6 detection and quadrilateral recognition on the shared ONNX Runtime.

mod det;
mod rec;

use lt_core::{
    engines::{TextLine, TextRecognizer},
    error::{Error, Result},
    screen::{filters, geom::RectF, line::RawLine, raster, vertical, Direction, ImageView},
};
use ort::{
    session::{builder::GraphOptimizationLevel, Session},
    value::TensorRef,
};
use std::{path::Path, time::Instant};

/// The manifest model: `det.onnx`, `rec.onnx` and `dict.txt`.
pub const OCR_MODEL: &str = "pp-ocrv6-det-tiny-rec-small";

pub struct OcrEngine {
    det: Session,
    rec: Session,
    dictionary: Vec<String>,
    classes: usize,
}

impl OcrEngine {
    /// `model_dir` is the OCR model's own folder. The caller has already called
    /// `lt_sherpa::initialize_shared_runtime`.
    pub fn load(model_dir: &Path, threads: usize) -> Result<Self> {
        let started = Instant::now();
        let dictionary = rec::dictionary(&std::fs::read_to_string(model_dir.join("dict.txt"))?);
        let det = session(&model_dir.join("det.onnx"), threads)?;
        let rec = session(&model_dir.join("rec.onnx"), threads)?;
        let classes = rec
            .outputs()
            .first()
            .and_then(|output| output.dtype().tensor_shape())
            .and_then(|shape| shape.last().copied())
            .unwrap_or(-1);
        if usize::try_from(classes) != Ok(dictionary.len() + 2) {
            return Err(Error::Engine(format!(
                "The text recognizer has {classes} classes, but its dictionary has {} entries",
                dictionary.len()
            )));
        }
        tracing::debug!(
            ms = started.elapsed().as_millis() as u64,
            "OCR model loaded"
        );
        Ok(Self {
            det,
            rec,
            classes: dictionary.len() + 2,
            dictionary,
        })
    }
}

impl TextRecognizer for OcrEngine {
    fn recognize(&mut self, image: &ImageView<'_>) -> Result<Vec<TextLine>> {
        self.recognize_direction(image, Direction::Horizontal)
    }

    fn recognize_direction(
        &mut self,
        image: &ImageView<'_>,
        direction: Direction,
    ) -> Result<Vec<TextLine>> {
        let vertical = direction == Direction::Vertical;
        let mut lines = self.read_pass(image, vertical, true, None)?;
        if vertical && image.width > 0 && image.height > 0 {
            let turned = vertical::turn(image);
            let upright_facts = vertical::prepare_lines(&lines);
            let upright: Vec<_> = upright_facts.iter().map(|l| l.bounds).collect();
            let additions =
                self.read_pass(&turned.view(), true, false, Some((&upright, image.width)))?;
            for mut line in additions {
                if lt_core::screen::line::glyphs_in(&line.text) < 2 {
                    continue;
                }
                let facts = vertical::prepare_lines(std::slice::from_ref(&line));
                let Some(fact) = facts.into_iter().next() else {
                    continue;
                };
                let fact = vertical::geometry::block_to_upright(fact, image.width);
                if vertical::repeated::says_what_was_already_read(&fact, &upright_facts) {
                    continue;
                }
                line.rect = fact.bounds.outward();
                line.quad = line.quad.map(|quad| {
                    quad.map(|p| {
                        lt_core::screen::geom::PointF::new(f64::from(image.width) - p.y, p.x)
                    })
                });
                lines.push(line);
            }
        }
        Ok(lines)
    }
}

impl OcrEngine {
    fn read_pass(
        &mut self,
        image: &ImageView<'_>,
        vertical: bool,
        keep_probability: bool,
        upright: Option<(&[RectF], u32)>,
    ) -> Result<Vec<TextLine>> {
        if image.width == 0 || image.height == 0 {
            return Ok(Vec::new());
        }
        let started = Instant::now();
        let geometry = det::Geometry::new(image.width, image.height);
        let frame = det::frame(image, &geometry);
        let input = det::input(&frame);
        let shape = [1, 3, geometry.input_h as usize, geometry.input_w as usize];
        let boxes = {
            let tensor = TensorRef::from_array_view((shape, &*input)).map_err(ort_error)?;
            let outputs = self.det.run(ort::inputs![tensor]).map_err(ort_error)?;
            let (map_shape, map) = outputs[0].try_extract_tensor::<f32>().map_err(ort_error)?;
            if map.len() != shape[2] * shape[3] {
                return Err(Error::Engine(format!(
                    "The text detector returned shape {map_shape:?} for input {shape:?}"
                )));
            }
            if vertical {
                let mut map = map.to_vec();
                if keep_probability {
                    vertical::columns_first::carve(
                        &mut map,
                        geometry.input_w as usize,
                        geometry.input_h as usize,
                        lt_core::screen::detection::BINARY,
                    );
                }
                let probability =
                    keep_probability.then_some(vertical::probability::DetectorProbability {
                        values: &map,
                        width: geometry.input_w as usize,
                        height: geometry.input_h as usize,
                        scale_x: 1.0,
                        scale_y: 1.0,
                        threshold: lt_core::screen::detection::BINARY,
                        score_threshold: lt_core::screen::detection::BOX_SCORE,
                        unclip_ratio: lt_core::screen::detection::UNCLIP,
                    });
                let boxes = lt_core::screen::detection::boxes(
                    &map,
                    geometry.input_w as usize,
                    geometry.input_h as usize,
                );
                vertical::detection::split(&frame.view(), &boxes, probability.as_ref())
            } else {
                det::boxes(map, &geometry, image)
            }
        };
        let detected = started.elapsed();

        let mut lines = Vec::new();
        let wanted = upright.map(|(upright, width)| {
            let turned: Vec<_> = boxes
                .iter()
                .map(|b| RectF::enclosing(&geometry.source_points(&b.points)))
                .collect();
            vertical::repeated::pieces_the_upright_pass_missed(upright, &turned, width)
        });
        for (index, detection) in boxes.iter().enumerate() {
            if wanted
                .as_ref()
                .is_some_and(|wanted| !wanted.contains(&index))
            {
                continue;
            }
            let points = if vertical {
                vertical::geometry::for_recognition(
                    vertical::detection::extend(&frame.view(), detection).points,
                )
            } else {
                detection.points
            };
            let crop = raster::crop(&frame.view(), &points);
            let width = rec::width(&crop);
            let input = rec::input(&crop);
            let shape = [1, 3, rec::HEIGHT as usize, width as usize];
            let tensor = TensorRef::from_array_view((shape, &*input)).map_err(ort_error)?;
            let outputs = self.rec.run(ort::inputs![tensor]).map_err(ort_error)?;
            let (_, probabilities) = outputs[0].try_extract_tensor::<f32>().map_err(ort_error)?;
            let (text, score, char_scores) =
                rec::decode(probabilities, self.classes, &self.dictionary);
            if score < rec::MIN_SCORE || char_scores.is_empty() {
                continue;
            }
            let points = geometry.source_points(&detection.points);
            let min_x = detection.points.iter().map(|p| p.0).min().unwrap();
            let max_x = detection.points.iter().map(|p| p.0).max().unwrap();
            let min_y = detection.points.iter().map(|p| p.1).min().unwrap();
            let max_y = detection.points.iter().map(|p| p.1).max().unwrap();
            if let Some(rect) =
                geometry.source_rect(min_x as f32, min_y as f32, max_x as f32, max_y as f32)
            {
                let line = TextLine {
                    rect,
                    text,
                    score,
                    quad: Some(points),
                    char_scores,
                    glyph_pitch: None,
                };
                lines.push(line);
            }
        }
        tracing::debug!(
            det_ms = detected.as_millis() as u64,
            total_ms = started.elapsed().as_millis() as u64,
            boxes = boxes.len(),
            lines = lines.len(),
            "OCR read an image"
        );
        Ok(if vertical {
            prepare_vertical_lines(lines)
        } else {
            lines
        })
    }
}

fn prepare_vertical_lines(lines: Vec<TextLine>) -> Vec<TextLine> {
    let mut prepared: Vec<_> = lines
        .into_iter()
        .map(|mut line| {
            let facts =
                vertical::geometry::prepare_blocks(filters::convert_blocks(&[RawLine::from(
                    &line,
                )]));
            line.glyph_pitch = facts.first().and_then(|fact| fact.render_glyph_height);
            line
        })
        .collect();
    prepared.sort_by(|a, b| {
        a.rect
            .y
            .cmp(&b.rect.y)
            .then_with(|| a.rect.x.cmp(&b.rect.x))
    });
    prepared
}

fn session(path: &Path, threads: usize) -> Result<Session> {
    Session::builder()
        .and_then(|builder| builder.with_intra_threads(threads).map_err(Into::into))
        .and_then(|builder| builder.with_inter_threads(1).map_err(Into::into))
        .and_then(|builder| builder.with_parallel_execution(false).map_err(Into::into))
        .and_then(|builder| {
            builder
                .with_optimization_level(GraphOptimizationLevel::All)
                .map_err(Into::into)
        })
        .and_then(|mut builder| builder.commit_from_file(path))
        .map_err(|error| {
            Error::Engine(format!(
                "Cannot open the OCR model {}: {error}",
                path.display()
            ))
        })
}

fn ort_error(error: ort::Error) -> Error {
    Error::Engine(format!("OCR failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::screen::Rect;

    #[test]
    fn vertical_pass_orders_readable_crops_before_coverage_checks() {
        let input =
            [(20, 12, "左"), (60, 10, "右"), (0, 0, "　"), (40, 10, "中")].map(|(x, y, text)| {
                TextLine {
                    rect: Rect { x, y, w: 20, h: 60 },
                    text: text.into(),
                    score: 0.99,
                    ..TextLine::default()
                }
            });
        let raw = prepare_vertical_lines(input.to_vec());
        assert_eq!(raw.len(), 4);
        let lines = vertical::prepare_lines(&raw);
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["中", "右", "左"]
        );
        assert!(lines.iter().all(|l| l.render_glyph_height == Some(20.0)));
    }
}
