//! The `lt-screen` thread: owns the text recognizer and runs one screen job at a time.
use crate::{
    app::{lock, Shared},
    controller,
};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use lt_core::{
    config::logical_cpus,
    error::{Error, Result},
    events::FailReason,
    screen::{piece_translator, run_job, ImageView, ScreenEvent, TranslationCache},
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
    pub id: u64,
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
    sender: Sender<Work>,
    waiting: Receiver<Work>,
    current: Mutex<Arc<AtomicBool>>,
    next: AtomicU64,
}

impl Jobs {
    pub fn new() -> (Self, Receiver<Work>) {
        let (sender, receiver) = bounded(1);
        let jobs = Self {
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
    pub fn start(&self, crop: Vec<u8>, width: u32, height: u32, origin: (i32, i32)) -> u64 {
        let mut current = lock(&self.current);
        current.store(true, Ordering::Release);
        let cancel = Arc::new(AtomicBool::new(false));
        *current = cancel.clone();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut work = Work::Run(Job {
            id,
            crop,
            width,
            height,
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
    }
}

struct Worker {
    shared: Arc<Shared>,
    engine: Option<OcrEngine>,
    cache: TranslationCache,
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
            cache,
            server,
            ..
        } = self;
        let engine = engine.as_mut().expect("loaded above");
        server.failed = None;
        let cancel = &job.cancel;
        let mut translate = |piece: &str| server.translate(piece, cancel);
        run_job(
            job.id,
            &image,
            engine,
            &mut translate,
            cancel,
            cache,
            &mut emit,
        );
    }
}

/// Moves crop rects to frame pixels.
fn to_frame(event: &mut ScreenEvent, (x, y): (i32, i32)) {
    if let ScreenEvent::Blocks { blocks, .. } = event {
        for block in blocks {
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
