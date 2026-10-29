//! PP-OCRv6 text recognition (detector and recognizer) on the shared ONNX Runtime. Horizontal
//! text only: no angle classifier, and every box is axis-aligned.

mod det;
mod image;
mod rec;

use lt_core::{
    engines::{TextLine, TextRecognizer},
    error::{Error, Result},
    screen::ImageView,
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
        let input = det::input(image, &geometry);
        let shape = [1, 3, geometry.input_h as usize, geometry.input_w as usize];
        let rects = {
            let tensor = TensorRef::from_array_view((shape, &*input)).map_err(ort_error)?;
            let outputs = self.det.run(ort::inputs![tensor]).map_err(ort_error)?;
            let (map_shape, map) = outputs[0].try_extract_tensor::<f32>().map_err(ort_error)?;
            if map.len() != shape[2] * shape[3] {
                return Err(Error::Engine(format!(
                    "The text detector returned shape {map_shape:?} for input {shape:?}"
                )));
            }
            det::boxes(map, &geometry)
        };
        let detected = started.elapsed();

        let mut order: Vec<usize> = (0..rects.len()).collect();
        order.sort_by_key(|&index| rec::width(&rects[index]));
        let mut lines = Vec::new();
        for batch in order.chunks(rec::BATCH) {
            let batch_width = rec::width(&rects[*batch.last().unwrap_or(&0)]);
            let slot = 3 * rec::HEIGHT as usize * batch_width as usize;
            let mut input = vec![0.0_f32; batch.len() * slot];
            for (&index, slot) in batch.iter().zip(input.chunks_exact_mut(slot)) {
                rec::fill(image, &rects[index], batch_width, slot);
            }
            let shape = [batch.len(), 3, rec::HEIGHT as usize, batch_width as usize];
            let tensor = TensorRef::from_array_view((shape, &*input)).map_err(ort_error)?;
            let outputs = self.rec.run(ort::inputs![tensor]).map_err(ort_error)?;
            let (_, probabilities) = outputs[0].try_extract_tensor::<f32>().map_err(ort_error)?;
            let per_crop = probabilities.len() / batch.len();
            for (&index, steps) in batch.iter().zip(probabilities.chunks_exact(per_crop)) {
                let (text, score) = rec::decode(steps, self.classes, &self.dictionary);
                let text = text.trim();
                if score >= rec::MIN_SCORE && !text.is_empty() {
                    lines.push(TextLine {
                        rect: rects[index],
                        text: text.to_owned(),
                        score,
                    });
                }
            }
        }
        tracing::debug!(
            det_ms = detected.as_millis() as u64,
            total_ms = started.elapsed().as_millis() as u64,
            boxes = rects.len(),
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
