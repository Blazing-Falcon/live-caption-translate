//! PP-OCRv6 detection and quadrilateral recognition on the shared ONNX Runtime.

mod det;
mod rec;

use lt_core::{
    engines::{TextLine, TextRecognizer},
    error::{Error, Result},
    screen::{raster, ImageView},
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
            det::boxes(map, &geometry, image)
        };
        let detected = started.elapsed();

        let mut lines = Vec::new();
        for detection in &boxes {
            let crop = raster::crop(&frame.view(), &detection.points);
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
                lines.push(TextLine {
                    rect,
                    text,
                    score,
                    quad: Some(points),
                    char_scores,
                });
            }
        }
        tracing::debug!(
            det_ms = detected.as_millis() as u64,
            total_ms = started.elapsed().as_millis() as u64,
            boxes = boxes.len(),
            lines = lines.len(),
            "OCR read an image"
        );
        Ok(lines)
    }
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
