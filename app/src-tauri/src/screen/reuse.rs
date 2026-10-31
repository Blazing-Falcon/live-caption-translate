use lt_core::{
    engines::{TextLine, TextRecognizer},
    error::Result,
    screen::{group::profile::CaptureLayoutMode, ImageView, Rect},
};

#[derive(Clone)]
pub struct RecognitionKey {
    pub frame: u64,
    pub region: Rect,
    pub language: String,
    pub vertical: bool,
    pub layout: CaptureLayoutMode,
}

#[derive(Default)]
pub struct RecognitionReuse {
    last: Option<(RecognitionKey, Vec<TextLine>)>,
}

fn normalized_language(code: &str) -> String {
    let code = code.trim();
    if code.is_empty() {
        "EN".into()
    } else {
        code.to_ascii_uppercase()
    }
}

impl RecognitionReuse {
    fn get(&self, key: &RecognitionKey) -> Option<&Vec<TextLine>> {
        self.last.as_ref().and_then(|(last, lines)| {
            (last.frame == key.frame
                && last.region == key.region
                && last.language == normalized_language(&key.language)
                && last.vertical == key.vertical
                && (key.vertical || last.layout == key.layout))
                .then_some(lines)
        })
    }

    fn remember(&mut self, mut key: RecognitionKey, lines: Vec<TextLine>) {
        key.language = normalized_language(&key.language);
        self.last = (!lines.is_empty()).then_some((key, lines));
    }

    pub fn clear(&mut self) {
        self.last = None;
    }

    pub fn reader<'a>(
        &'a mut self,
        key: RecognitionKey,
        engine: &'a mut dyn TextRecognizer,
    ) -> ReusingReader<'a> {
        ReusingReader {
            reuse: self,
            key,
            engine,
        }
    }
}

pub struct ReusingReader<'a> {
    reuse: &'a mut RecognitionReuse,
    key: RecognitionKey,
    engine: &'a mut dyn TextRecognizer,
}

impl TextRecognizer for ReusingReader<'_> {
    fn recognize(&mut self, image: &ImageView<'_>) -> Result<Vec<TextLine>> {
        if let Some(lines) = self.reuse.get(&self.key) {
            return Ok(lines.clone());
        }
        let direction = if self.key.vertical {
            lt_core::screen::Direction::Vertical
        } else {
            lt_core::screen::Direction::Horizontal
        };
        let lines = self.engine.recognize_direction(image, direction)?;
        self.reuse.remember(self.key.clone(), lines.clone());
        Ok(lines)
    }
    fn recognize_direction(
        &mut self,
        image: &ImageView<'_>,
        _direction: lt_core::screen::Direction,
    ) -> Result<Vec<TextLine>> {
        self.recognize(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> RecognitionKey {
        RecognitionKey {
            frame: 1,
            region: Rect {
                x: 100,
                y: 200,
                w: 300,
                h: 40,
            },
            language: "ja".into(),
            vertical: false,
            layout: CaptureLayoutMode::General,
        }
    }

    fn found() -> Vec<TextLine> {
        vec![TextLine {
            text: "HELLO".into(),
            rect: Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            ..Default::default()
        }]
    }

    fn remembering() -> RecognitionReuse {
        let mut reuse = RecognitionReuse::default();
        reuse.remember(key(), found());
        reuse
    }

    #[test]
    fn the_same_crop_asked_the_same_way_is_reused() {
        let reuse = remembering();
        assert!(std::ptr::eq(
            reuse.get(&key()).unwrap(),
            &reuse.last.as_ref().unwrap().1
        ));
    }

    #[test]
    fn the_same_region_cut_again_is_reused() {
        let mut again = key();
        again.region = Rect {
            x: 100,
            y: 200,
            w: 300,
            h: 40,
        };
        assert!(remembering().get(&again).is_some());
    }

    #[test]
    fn the_same_language_written_differently_is_reused() {
        let mut again = key();
        again.language = " JA ".into();
        assert!(remembering().get(&again).is_some());
    }

    #[test]
    fn another_region_is_recognized_again() {
        let mut again = key();
        again.region.x += 1;
        assert!(remembering().get(&again).is_none());
    }

    #[test]
    fn another_capture_is_recognized_again() {
        let mut again = key();
        again.frame += 1;
        assert!(remembering().get(&again).is_none());
    }

    #[test]
    fn another_source_language_is_recognized_again() {
        let mut again = key();
        again.language = "en".into();
        assert!(remembering().get(&again).is_none());
    }

    #[test]
    fn another_orientation_is_recognized_again() {
        let mut again = key();
        again.vertical = true;
        assert!(remembering().get(&again).is_none());
    }

    #[test]
    fn another_layout_mode_is_recognized_again() {
        let mut again = key();
        again.layout = CaptureLayoutMode::Interface;
        assert!(remembering().get(&again).is_none());
    }

    #[test]
    fn another_layout_mode_on_vertical_text_is_reused() {
        let mut first = key();
        first.vertical = true;
        let mut reuse = RecognitionReuse::default();
        reuse.remember(first.clone(), found());
        first.layout = CaptureLayoutMode::Interface;
        assert!(reuse.get(&first).is_some());
    }

    #[test]
    fn after_clear_nothing_is_reused() {
        let mut reuse = remembering();
        reuse.clear();
        assert!(reuse.get(&key()).is_none());
    }

    #[test]
    fn a_recognition_that_found_nothing_is_not_kept() {
        let mut reuse = remembering();
        reuse.remember(key(), vec![]);
        assert!(reuse.get(&key()).is_none());
    }

    #[test]
    fn repeated_reads_skip_the_engine_but_a_changed_crop_reads_again() {
        struct Reader(usize);
        impl TextRecognizer for Reader {
            fn recognize(&mut self, _: &ImageView<'_>) -> Result<Vec<TextLine>> {
                self.0 += 1;
                Ok(found())
            }
        }
        let mut engine = Reader(0);
        let mut reuse = RecognitionReuse::default();
        let image = ImageView::new(1, 1, 4, &[0, 0, 0, 255]).unwrap();
        for _ in 0..2 {
            assert_eq!(
                reuse.reader(key(), &mut engine).recognize(&image).unwrap(),
                found()
            );
        }
        assert_eq!(engine.0, 1);
        let mut moved = key();
        moved.region.x += 1;
        reuse.reader(moved, &mut engine).recognize(&image).unwrap();
        assert_eq!(engine.0, 2);
    }
}
