use crate::app::lock;
use lt_core::screen::{
    erase::RemovalBlock,
    geom::RectF,
    line::Line,
    paint::backdrop::{self, Backdrop},
    ImageView,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Deserialize)]
pub struct PlateRequest {
    pub frame: u64,
    pub job: u64,
    pub block: u32,
    pub rect: RectF,
    pub written_height: f64,
}

#[derive(Serialize)]
pub struct PlateReply {
    pub rect: RectF,
    pub foreground: [u8; 3],
    pub url: String,
}

struct SourcePaint {
    background: [u8; 3],
    text: [u8; 3],
    glyph_height: f64,
}

struct Image {
    revision: u64,
    png: Vec<u8>,
}

struct Prepared {
    backdrop: Option<Backdrop>,
    blocks: Vec<SourcePaint>,
}

struct Current {
    frame: u64,
    job: u64,
    origin: (i32, i32),
    cancel: Arc<AtomicBool>,
    prepared: Option<Arc<Prepared>>,
    next_revision: u64,
    revisions: BTreeMap<u32, u64>,
    images: BTreeMap<u32, Image>,
}

#[derive(Default)]
pub struct Plates(Mutex<Option<Current>>);

impl Plates {
    pub fn begin(&self, frame: u64, job: u64, origin: (i32, i32), cancel: Arc<AtomicBool>) {
        *lock(&self.0) = Some(Current {
            frame,
            job,
            origin,
            cancel,
            prepared: None,
            next_revision: 1,
            revisions: BTreeMap::new(),
            images: BTreeMap::new(),
        });
    }

    pub fn clear(&self) {
        *lock(&self.0) = None;
    }

    pub fn prepare(
        &self,
        job: u64,
        image: &ImageView<'_>,
        groups: &[Line],
        colors: &[lt_core::screen::Block],
        cancel: &AtomicBool,
    ) -> lt_core::error::Result<()> {
        let blocks = groups
            .iter()
            .zip(colors)
            .map(|(line, block)| {
                let mut heights: Vec<_> = line.lines().iter().map(|r| r.height).collect();
                heights.sort_by(f64::total_cmp);
                let height = line
                    .render_glyph_height
                    .filter(|h| *h > 0.0)
                    .unwrap_or_else(|| {
                        heights
                            .get(heights.len() / 2)
                            .copied()
                            .unwrap_or(line.bounds.height)
                    });
                SourcePaint {
                    background: block.background,
                    text: block.foreground,
                    glyph_height: height,
                }
            })
            .collect();
        let removal: Vec<_> = groups
            .iter()
            .map(|line| RemovalBlock {
                translated_text: "x".into(),
                bounds: line.bounds,
                source_line_bounds: line.source_line_bounds.clone(),
                glyph_height: line.render_glyph_height,
                within: line.tilt.as_ref().map(|tilt| tilt.erase_quads()),
            })
            .collect();
        let backdrop = Backdrop::create(*image, &removal, cancel)?;
        let mut state = lock(&self.0);
        if let Some(current) = state
            .as_mut()
            .filter(|c| c.job == job && !c.cancel.load(Ordering::Acquire))
        {
            current.prepared = Some(Arc::new(Prepared { backdrop, blocks }));
        }
        Ok(())
    }

    pub fn request(&self, request: PlateRequest) -> Result<PlateReply, String> {
        if ![
            request.rect.x,
            request.rect.y,
            request.rect.width,
            request.rect.height,
            request.written_height,
        ]
        .into_iter()
        .all(f64::is_finite)
            || request.rect.width <= 0.0
            || request.rect.height <= 0.0
            || request.written_height < 0.0
        {
            return Err("The text layout is empty or invalid.".into());
        }
        let (prepared, origin, revision) = {
            let mut state = lock(&self.0);
            let current = state
                .as_mut()
                .filter(|c| {
                    c.frame == request.frame
                        && c.job == request.job
                        && !c.cancel.load(Ordering::Acquire)
                })
                .ok_or("This screenshot job is closed.")?;
            let prepared = current
                .prepared
                .clone()
                .ok_or("This screenshot is still being read.")?;
            if request.block as usize >= prepared.blocks.len() {
                return Err("This text block is closed.".into());
            }
            let revision = current.next_revision;
            current.next_revision += 1;
            current.revisions.insert(request.block, revision);
            (prepared, current.origin, revision)
        };
        let bubble = request
            .rect
            .offset(-f64::from(origin.0), -f64::from(origin.1));
        let block = &prepared.blocks[request.block as usize];
        let feather = backdrop::feather(block.glyph_height);
        let area = bubble.inflate(feather, feather);
        let plate = prepared.backdrop.as_ref().and_then(|b| {
            b.plate(
                area,
                block.background,
                block.text,
                block.glyph_height,
                request.written_height,
                None,
            )
        });
        let (rect, text, width, height, pixels) = if let Some(plate) = plate {
            (area, plate.text, plate.width, plate.height, plate.bgra)
        } else {
            let (background, text) = prepared
                .backdrop
                .as_ref()
                .and_then(|b| b.card(bubble, block.text))
                .unwrap_or((block.background, block.text));
            let (w, h) = (
                bubble.width.ceil().max(1.0) as usize,
                bubble.height.ceil().max(1.0) as usize,
            );
            let mut pixels = vec![0; w * h * 4];
            for p in pixels.as_chunks_mut::<4>().0 {
                p.copy_from_slice(&[background[2], background[1], background[0], 255]);
            }
            (bubble, text, w, h, pixels)
        };
        let png = encode(width, height, &pixels)?;
        let mut state = lock(&self.0);
        let current = state
            .as_mut()
            .filter(|c| {
                c.frame == request.frame
                    && c.job == request.job
                    && !c.cancel.load(Ordering::Acquire)
                    && c.revisions.get(&request.block) == Some(&revision)
            })
            .ok_or("This text layout was replaced.")?;
        current
            .images
            .insert(request.block, Image { revision, png });
        Ok(PlateReply {
            rect: rect.offset(f64::from(origin.0), f64::from(origin.1)),
            foreground: text,
            url: format!(
                "http://ltshot.localhost/{}/plate/{}/{}?v={revision}",
                request.frame, request.job, request.block
            ),
        })
    }

    pub fn image(&self, path: &str, query: Option<&str>) -> Option<Vec<u8>> {
        let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if parts.len() != 4 || parts[1] != "plate" {
            return None;
        }
        let (frame, job, block) = (
            parts[0].parse::<u64>().ok()?,
            parts[2].parse::<u64>().ok()?,
            parts[3].parse::<u32>().ok()?,
        );
        let revision = query?.strip_prefix("v=")?.parse::<u64>().ok()?;
        let state = lock(&self.0);
        let current = state
            .as_ref()
            .filter(|c| c.frame == frame && c.job == job && !c.cancel.load(Ordering::Acquire))?;
        let image = current.images.get(&block)?;
        (image.revision == revision && current.revisions.get(&block) == Some(&revision))
            .then(|| image.png.clone())
    }
}

fn encode(width: usize, height: usize, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let rgba: Vec<_> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    let mut result = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut result, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared() -> (Plates, Arc<AtomicBool>) {
        let store = Plates::default();
        let cancel = Arc::new(AtomicBool::new(false));
        store.begin(5, 8, (50, 70), cancel.clone());
        let pixels = vec![255; 8 * 8 * 4];
        let image = ImageView::new(8, 8, 32, &pixels).unwrap();
        let line = Line {
            text: "中文".into(),
            bounds: RectF::new(2.0, 2.0, 4.0, 2.0),
            ..Line::default()
        };
        let groups = [line];
        let blocks = lt_core::screen::group::blocks_from_groups(&image, &groups);
        store.prepare(8, &image, &groups, &blocks, &cancel).unwrap();
        (store, cancel)
    }

    fn request() -> PlateRequest {
        PlateRequest {
            frame: 5,
            job: 8,
            block: 0,
            rect: RectF::new(51.0, 71.0, 4.0, 4.0),
            written_height: 2.0,
        }
    }

    #[test]
    fn current_job_plate_uses_frame_coordinates_and_an_opaque_png_card() {
        let (store, _) = prepared();
        let reply = store.request(request()).unwrap();
        assert_eq!(reply.rect, request().rect);
        assert_eq!(reply.foreground, [0; 3]);
        assert_eq!(reply.url, "http://ltshot.localhost/5/plate/8/0?v=1");
        let png = store.image("/5/plate/8/0", Some("v=1")).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height), (4, 4));
        assert!(pixels[..info.buffer_size()].iter().all(|v| *v == 255));
        assert!(store.image("/5/plate/8/0", None).is_none());
        assert!(store.image("/6/plate/8/0", Some("v=1")).is_none());
        assert!(store.image("/5/plate/9/0", Some("v=1")).is_none());
    }

    #[test]
    fn a_new_layout_invalidates_the_previous_image_revision() {
        let (store, _) = prepared();
        store.request(request()).unwrap();
        let mut next = request();
        next.rect.width = 5.0;
        let reply = store.request(next).unwrap();
        assert_eq!(reply.rect.width, 5.0);
        assert!(store.image("/5/plate/8/0", Some("v=1")).is_none());
        assert!(store.image("/5/plate/8/0", Some("v=2")).is_some());
        assert_eq!(lock(&store.0).as_ref().unwrap().images.len(), 1);
    }

    #[test]
    fn cancellation_and_replacement_reject_plate_requests_and_urls() {
        let (store, cancel) = prepared();
        store.request(request()).unwrap();
        cancel.store(true, Ordering::Release);
        assert!(store.request(request()).is_err());
        assert!(store.image("/5/plate/8/0", Some("v=1")).is_none());
        store.begin(5, 9, (50, 70), Arc::new(AtomicBool::new(false)));
        assert!(store.request(request()).is_err());
        assert!(store.image("/5/plate/8/0", Some("v=1")).is_none());
        store.clear();
        assert!(lock(&store.0).is_none());
    }
}
