//! The `lt-screen` thread: owns the text recognizer and runs one screen job at a time.
use super::reuse::{RecognitionKey, RecognitionReuse};
use crate::{
    app::{lock, Shared},
    controller,
};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use lt_core::screen::{group::profile::CaptureLayoutMode, Rect};
use lt_core::{
    config::logical_cpus,
    error::{Error, Result},
    events::FailReason,
    screen::{
        piece_translator, run_job_with_output, ImageView, JobOutput, ScreenEvent, TranslationCache,
    },
};
use lt_llm::{models::ModelState, OpenAiCompatTranslator};
use lt_ocr::{OcrEngine, OCR_MODEL};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tauri::Emitter;

/// The recognizer's memory is returned after this long without a job.
const IDLE: Duration = Duration::from_secs(5 * 60);

pub struct Job {
    pub direction: lt_core::screen::Direction,
    pub id: u64,
    pub frame: u64,
    pub debug: bool,
    /// BGRA rows of `width * 4` bytes, cut from the frame at `origin`.
    pub crop: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub origin: (i32, i32),
    pub cancel: Arc<AtomicBool>,
}

pub enum Work {
    Load,
    Run(Job),
}

/// The sending side. The channel holds one waiting item; a new job replaces a waiting one.
pub struct Jobs {
    pub plates: super::plates::Plates,
    sender: Sender<Work>,
    waiting: Receiver<Work>,
    current: Mutex<Arc<AtomicBool>>,
    next: AtomicU64,
}

impl Jobs {
    pub fn new() -> (Self, Receiver<Work>) {
        let (sender, receiver) = bounded(1);
        let jobs = Self {
            plates: super::plates::Plates::default(),
            sender,
            waiting: receiver.clone(),
            current: Mutex::default(),
            next: AtomicU64::new(1),
        };
        (jobs, receiver)
    }

    /// Loads the recognizer ahead of the first job, unless work is already waiting.
    pub fn preload(&self) {
        let _ = self.sender.try_send(Work::Load);
    }

    /// Cancels the previous job and queues this one.
    pub fn start(
        &self,
        frame: u64,
        crop: Vec<u8>,
        area: Rect,
        debug: bool,
        direction: lt_core::screen::Direction,
    ) -> u64 {
        let mut current = lock(&self.current);
        current.store(true, Ordering::Release);
        let cancel = Arc::new(AtomicBool::new(false));
        *current = cancel.clone();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let origin = (area.x, area.y);
        self.plates.begin(frame, id, origin, cancel.clone());
        let mut work = Work::Run(Job {
            direction,
            id,
            frame,
            debug,
            crop,
            width: area.w,
            height: area.h,
            origin,
            cancel,
        });
        loop {
            match self.sender.try_send(work) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(back)) => {
                    let _ = self.waiting.try_recv();
                    work = back;
                }
            }
        }
        id
    }

    pub fn cancel(&self) {
        lock(&self.current).store(true, Ordering::Release);
        self.plates.clear();
    }
}

struct Worker {
    shared: Arc<Shared>,
    engine: Option<OcrEngine>,
    cache: TranslationCache,
    reuse: RecognitionReuse,
    server: Server,
}

pub fn spawn(shared: Arc<Shared>, work: Receiver<Work>) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("lt-screen".into())
        .spawn(move || {
            Worker {
                server: Server {
                    shared: shared.clone(),
                    client: None,
                    failed: None,
                },
                shared,
                engine: None,
                cache: TranslationCache::default(),
                reuse: RecognitionReuse::default(),
            }
            .run(work)
        })
}

impl Worker {
    fn run(mut self, work: Receiver<Work>) {
        loop {
            let next = if self.engine.is_some() {
                match work.recv_timeout(IDLE) {
                    Ok(next) => next,
                    Err(RecvTimeoutError::Timeout) => {
                        self.engine = None;
                        self.reuse.clear();
                        tracing::debug!("OCR model unloaded after idle time");
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match work.recv() {
                    Ok(next) => next,
                    Err(_) => return,
                }
            };
            match next {
                Work::Load => {
                    if let Err(message) = self.load() {
                        tracing::debug!(message, "OCR model not loaded ahead");
                    }
                }
                Work::Run(job) => self.run_job(job),
            }
        }
    }

    fn load(&mut self) -> std::result::Result<(), String> {
        if self.engine.is_some() {
            return Ok(());
        }
        let models = self.shared.paths.models(&self.shared.config());
        let ready = lt_llm::models::status(&models).is_ok_and(|models| {
            models
                .iter()
                .any(|model| model.id == OCR_MODEL && model.state == ModelState::Ready)
        });
        if !ready {
            return Err("Screen translation needs the text recognition model.".into());
        }
        let threads = (logical_cpus() as usize / 2).clamp(1, 4);
        lt_sherpa::default_runtime_path()
            .and_then(lt_sherpa::initialize_shared_runtime)
            .and_then(|()| OcrEngine::load(&models.join(OCR_MODEL), threads))
            .map(|engine| self.engine = Some(engine))
            .map_err(|error| format!("The text recognizer could not load: {error}"))
    }

    fn run_job(&mut self, job: Job) {
        if job.cancel.load(Ordering::Acquire) {
            return;
        }
        let shared = self.shared.clone();
        let origin = job.origin;
        let mut emit = move |mut event: ScreenEvent| {
            to_frame(&mut event, origin);
            let _ = shared
                .handle
                .emit_to(super::LABEL, "screen://event", &event);
        };
        let loaded = self.load().and_then(|()| {
            ImageView::new(job.width, job.height, job.width * 4, &job.crop)
                .map_err(|error| error.to_string())
        });
        let image = match loaded {
            Ok(image) => image,
            Err(_) if job.cancel.load(Ordering::Acquire) => return,
            Err(message) => {
                emit(ScreenEvent::Reading { job: job.id });
                emit(ScreenEvent::Error {
                    job: job.id,
                    message,
                });
                return;
            }
        };
        let Self {
            engine,
            reuse,
            cache,
            server,
            ..
        } = self;
        let engine = engine.as_mut().expect("loaded above");
        let mut reader = reuse.reader(
            RecognitionKey {
                frame: job.frame,
                region: Rect {
                    x: job.origin.0,
                    y: job.origin.1,
                    w: job.width,
                    h: job.height,
                },
                language: "ZH".into(),
                vertical: job.direction == lt_core::screen::Direction::Vertical,
                layout: CaptureLayoutMode::General,
            },
            engine,
        );
        server.failed = None;
        let cancel = &job.cancel;
        let mut translate = |piece: &str| server.translate(piece, cancel);
        let plates = &self.shared.screen.jobs.plates;
        let mut prepare = |image: &ImageView<'_>,
                           groups: &[lt_core::screen::line::Line],
                           blocks: &[lt_core::screen::Block]| {
            plates.prepare(job.id, image, groups, blocks, cancel)
        };
        run_job_with_output(
            job.id,
            &image,
            &mut reader,
            &mut translate,
            cancel,
            cache,
            JobOutput {
                direction: job.direction,
                emit: &mut emit,
                prepare: Some(&mut prepare),
                debug: job.debug,
            },
        );
    }
}

/// Moves crop rects to frame pixels.
fn to_frame(event: &mut ScreenEvent, (x, y): (i32, i32)) {
    if let ScreenEvent::Blocks {
        blocks, ocr_lines, ..
    } = event
    {
        if let Some(lines) = ocr_lines {
            for rect in lines {
                rect.x += x;
                rect.y += y;
            }
        }
        for block in blocks {
            if let Some(tilt) = &block.tilt {
                block.tilt = Some(tilt.offset(f64::from(x), f64::from(y)));
            }
            block.paint.bounds = block.paint.bounds.offset(f64::from(x), f64::from(y));
            for line in &mut block.paint.source_lines {
                *line = line.offset(f64::from(x), f64::from(y));
            }
            for rect in std::iter::once(&mut block.rect).chain(&mut block.lines) {
                rect.x += x;
                rect.y += y;
            }
        }
    }
}

/// A client of the translation server the controller owns.
struct Server {
    shared: Arc<Shared>,
    client: Option<OpenAiCompatTranslator>,
    /// Why the server could not start in this job; later blocks fail at once.
    failed: Option<String>,
}

impl Server {
    fn translate(&mut self, piece: &str, cancel: &Arc<AtomicBool>) -> Result<String> {
        let first = piece_translator(self.client(false)?, cancel)(piece);
        match first {
            // A server the supervisor gave up on is replaced by one on a new port. A crash it
            // restarts keeps the port, so blocks sent during that restart fail.
            Err(Error::Translation {
                reason: FailReason::ServerUnavailable,
                ..
            }) => piece_translator(self.client(true)?, cancel)(piece),
            other => other,
        }
    }

    fn client(&mut self, fresh: bool) -> Result<&mut OpenAiCompatTranslator> {
        if let Some(message) = &self.failed {
            return Err(Error::Engine(message.clone()));
        }
        if fresh || self.client.is_none() {
            self.client = None;
            let url = controller::server_url(&self.shared).inspect_err(|message| {
                self.failed = Some(message.clone());
            });
            self.client = Some(OpenAiCompatTranslator::new(&url.map_err(Error::Engine)?)?);
        }
        Ok(self.client.as_mut().expect("connected above"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilted_card_geometry_receives_the_crop_origin_once() {
        let events: Vec<ScreenEvent> = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ui/src/lib/__fixtures__/screen-events.json"
        )))
        .unwrap();
        let mut event = events
            .into_iter()
            .find(|e| matches!(e, ScreenEvent::Blocks { .. }))
            .unwrap();
        to_frame(&mut event, (100, 200));
        let ScreenEvent::Blocks { blocks, .. } = event else {
            unreachable!()
        };
        let shape = blocks[2].tilt.as_ref().unwrap();
        assert_eq!(
            shape.centre,
            lt_core::screen::geom::PointF::new(228.0, 381.0)
        );
        assert_eq!(
            shape.box_rect,
            lt_core::screen::geom::RectF::new(188.0, 376.0, 80.0, 10.0)
        );
        assert_eq!(
            shape.line_quads[0][0],
            lt_core::screen::geom::PointF::new(189.0, 371.0)
        );
        assert_eq!(shape.degrees, 8.0);
        assert!(blocks[0].tilt.is_none());
    }

    #[test]
    fn raw_debug_boxes_receive_the_crop_origin_once() {
        let mut event = ScreenEvent::Blocks {
            job: 1,
            blocks: vec![],
            ocr_ms: 0,
            ocr_lines: Some(vec![Rect {
                x: 10,
                y: 20,
                w: 30,
                h: 40,
            }]),
        };
        to_frame(&mut event, (100, 200));
        let ScreenEvent::Blocks { ocr_lines, .. } = event else {
            unreachable!()
        };
        assert_eq!(
            ocr_lines.unwrap(),
            [Rect {
                x: 110,
                y: 220,
                w: 30,
                h: 40
            }]
        );
    }
}
