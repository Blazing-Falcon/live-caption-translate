use crate::args::ScreenArgs;
#[cfg(not(feature = "ocr"))]
use anyhow::bail;
use anyhow::Result;
use lt_core::config::Config;
use std::sync::{atomic::AtomicBool, Arc};

#[cfg(not(feature = "ocr"))]
pub fn screen(_: ScreenArgs, _: Config, _: Arc<AtomicBool>) -> Result<()> {
    bail!("Reading a screenshot requires --features ocr (and llm to translate it)");
}

#[cfg(feature = "ocr")]
pub fn screen(args: ScreenArgs, config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    run::screen(args, config, cancelled)
}

#[cfg(feature = "ocr")]
mod run {
    use crate::{args::ScreenArgs, engines};
    use anyhow::{Context, Result};
    use lt_core::{
        bus::EventBus,
        config::{logical_cpus, Config},
        engines::{TextLine, TextRecognizer, Translator},
        error::Error,
        screen::{
            piece_translator, run_job_with_output, Direction, ImageView, JobOutput, ScreenEvent,
            TranslationCache,
        },
    };
    use std::{
        fs::{self, File},
        io::BufReader,
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Instant,
    };

    struct Server {
        translator: Box<dyn Translator>,
        // Kills the child when dropped.
        _supervisor: lt_llm::supervisor::Supervisor,
    }

    /// Keeps the lines the engine found, which the job reports only as blocks.
    struct Recorder<'a> {
        engine: &'a mut dyn TextRecognizer,
        lines: Vec<TextLine>,
    }

    impl TextRecognizer for Recorder<'_> {
        fn recognize(&mut self, image: &ImageView<'_>) -> lt_core::error::Result<Vec<TextLine>> {
            self.lines = self.engine.recognize(image)?;
            Ok(self.lines.clone())
        }
        fn recognize_direction(
            &mut self,
            image: &ImageView<'_>,
            direction: Direction,
        ) -> lt_core::error::Result<Vec<TextLine>> {
            self.lines = self.engine.recognize_direction(image, direction)?;
            Ok(self.lines.clone())
        }
    }

    pub fn screen(args: ScreenArgs, config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
        let models = engines::models_dir(args.models.as_deref(), &config);
        let (width, height, bgra) = decode(&args.png)?;
        let image = ImageView::new(width, height, width * 4, &bgra)?;
        lt_sherpa::initialize_shared_runtime(lt_sherpa::default_runtime_path()?)?;
        let threads = args
            .threads
            .map_or_else(|| (logical_cpus() as usize / 2).clamp(1, 4), usize::from);
        let loading = Instant::now();
        let mut engine = lt_ocr::OcrEngine::load(&models.join(lt_ocr::OCR_MODEL), threads)
            .context("Loading the text recognizer")?;
        let ocr_load_ms = millis(loading);
        let loading = Instant::now();
        let mut server = start_server(&args, &config, &models, &cancelled)?;
        let server_ms = server.as_ref().map(|_| millis(loading));

        let mut recorder = Recorder {
            engine: &mut engine,
            lines: Vec::new(),
        };
        let mut cache = TranslationCache::default();
        let started = Instant::now();
        let mut events: Vec<(u32, ScreenEvent)> = Vec::new();
        let mut emit = |event: ScreenEvent| {
            // Without a translator the job ends once the blocks are known.
            if args.no_translate && matches!(event, ScreenEvent::Blocks { .. }) {
                cancelled.store(true, Ordering::Release);
            }
            events.push((millis(started), event));
        };
        match server.as_mut() {
            Some(server) => run_job_with_output(
                1,
                &image,
                &mut recorder,
                &mut piece_translator(&mut *server.translator, &cancelled),
                &cancelled,
                &mut cache,
                JobOutput {
                    direction: args.direction.into(),
                    debug: false,
                    prepare: None,
                    emit: &mut emit,
                },
            ),
            None => run_job_with_output(
                1,
                &image,
                &mut recorder,
                &mut |_| Err(Error::Stopped),
                &cancelled,
                &mut cache,
                JobOutput {
                    direction: args.direction.into(),
                    debug: false,
                    prepare: None,
                    emit: &mut emit,
                },
            ),
        }
        let lines = recorder.lines;
        print_table(&events);
        println!(
            "OCR {} ms (model load {ocr_load_ms} ms, {threads} threads), {} lines",
            events
                .iter()
                .find_map(|(_, event)| match event {
                    ScreenEvent::Blocks { ocr_ms, .. } => Some(*ocr_ms),
                    _ => None,
                })
                .map_or("-".into(), |ms| ms.to_string()),
            lines.len(),
        );
        if let Some(ms) = server_ms {
            println!("Translator ready in {ms} ms");
        }

        if let Some(parent) = args
            .out
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let name = args
            .png
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let mut output = serde_json::json!({
            "image": name,
            "width": width,
            "height": height,
            "direction": Direction::from(args.direction),
            "ocr_load_ms": ocr_load_ms,
            "server_ms": server_ms,
            "lines": lines,
            "events": events
                .iter()
                .map(|(t_ms, event)| serde_json::json!({"t_ms": t_ms, "event": event}))
                .collect::<Vec<_>>(),
        });
        if args.explain && Direction::from(args.direction) == Direction::Horizontal {
            use lt_core::screen::{filters, group, ink, line::RawLine};
            let raw: Vec<RawLine> = lines.iter().map(RawLine::from).collect();
            let mut facts = filters::prepare(&raw);
            ink::annotate(&image, &mut facts);
            let grouping = group::report_screenshot(&facts, &group::profile::GENERAL, true);
            output["decisions"] = serde_json::to_value(grouping.decisions)?;
            output["grouping_trace"] = serde_json::to_value(grouping.trace)?;
        }
        let file =
            File::create(&args.out).with_context(|| format!("Creating {}", args.out.display()))?;
        serde_json::to_writer_pretty(file, &output)?;
        Ok(())
    }

    fn start_server(
        args: &ScreenArgs,
        config: &Config,
        models: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Option<Server>> {
        if args.no_translate {
            return Ok(None);
        }
        let (translator, supervisor, _) = engines::translator(
            config,
            models,
            args.server.as_deref(),
            &EventBus::default(),
            cancelled,
        )?;
        Ok(Some(Server {
            translator,
            _supervisor: supervisor,
        }))
    }

    /// One row per block: id, source, English, milliseconds since the previous event.
    fn print_table(events: &[(u32, ScreenEvent)]) {
        let mut blocks = Vec::new();
        let mut previous = 0;
        let mut rows: Vec<(u32, String, u32)> = Vec::new();
        for (t_ms, event) in events {
            match event {
                ScreenEvent::Blocks { blocks: found, .. } => {
                    blocks.clone_from(found);
                    previous = *t_ms;
                }
                ScreenEvent::Translated { id, text, .. } => {
                    rows.push((*id, text.clone(), t_ms - previous));
                    previous = *t_ms;
                }
                ScreenEvent::Failed { id, message, .. } => {
                    rows.push((*id, format!("FAILED: {message}"), t_ms - previous));
                    previous = *t_ms;
                }
                ScreenEvent::Error { message, .. } => println!("Error: {message}"),
                ScreenEvent::Reading { .. } | ScreenEvent::Done { .. } => {}
            }
        }
        println!("id\tsource\tEnglish\tms");
        for block in &blocks {
            let row = rows.iter().find(|(id, ..)| *id == block.id);
            println!(
                "{}\t{}\t{}\t{}",
                block.id,
                block.source.replace('\n', "⏎"),
                row.map_or("", |(_, text, _)| text.as_str()),
                row.map_or(String::new(), |(.., ms)| ms.to_string()),
            );
        }
        if let Some((_, ScreenEvent::Done { total_ms, .. })) = events.last() {
            println!("Done in {total_ms} ms");
        }
    }

    fn millis(started: Instant) -> u32 {
        u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
    }

    /// The image as BGRA with every pixel opaque.
    fn decode(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
        let file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
        let mut decoder = png::Decoder::new(BufReader::new(file));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder
            .read_info()
            .with_context(|| format!("Reading {}", path.display()))?;
        let mut buffer = vec![
            0;
            reader
                .output_buffer_size()
                .context("The image is too large")?
        ];
        let info = reader.next_frame(&mut buffer)?;
        let channels = info.color_type.samples();
        let bgra = buffer[..info.buffer_size()]
            .chunks_exact(channels)
            .flat_map(|pixel| match channels {
                1 | 2 => [pixel[0], pixel[0], pixel[0], 255],
                _ => [pixel[2], pixel[1], pixel[0], 255],
            })
            .collect();
        Ok((info.width, info.height, bgra))
    }
}
