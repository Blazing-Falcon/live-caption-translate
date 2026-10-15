//! SenseVoice: it reads each window whole, so it hears each word with the words after it, which
//! the streaming model cannot.

use crate::{
    offline::{zeroed_config, OfflineRecognizer},
    runtime::{cstring, path_cstring, prepare},
};
use lt_core::{
    config::AsrConfig,
    engines::{OfflineAsr, OfflineText},
    error::Result,
};
use std::path::{Path, PathBuf};

const NAME: &str = "SenseVoice";

pub struct SenseVoice {
    model: PathBuf,
    tokens: PathBuf,
    threads: u32,
    detect: OfflineRecognizer,
    /// sherpa reads SenseVoice's language when the recognizer is created (a stream option does
    /// not change it), so a forced language needs its own recognizer. It is created on first
    /// use: few windows need one, and each recognizer holds its own copy of the model.
    forced: Option<(String, OfflineRecognizer)>,
}

impl SenseVoice {
    /// `dir` holds the model and tokens of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, settings: &AsrConfig) -> Result<Self> {
        let dir = dir.as_ref();
        let model = dir.join("model.int8.onnx");
        let tokens = dir.join("tokens.txt");
        prepare(NAME, None, &[&model, &tokens])?;
        let threads = settings.second_pass_engine_threads();
        let detect = recognizer(&model, &tokens, "auto", threads)?;
        Ok(Self {
            model,
            tokens,
            threads,
            detect,
            forced: None,
        })
    }
}

impl OfflineAsr for SenseVoice {
    fn decode(&mut self, samples: &[f32], language: Option<&str>) -> Result<OfflineText> {
        let Some(language) = language else {
            return self.detect.decode(samples, None);
        };
        let forced = match self.forced.take() {
            Some(forced) if forced.0 == language => forced,
            _ => (
                language.to_owned(),
                recognizer(&self.model, &self.tokens, language, self.threads)?,
            ),
        };
        let heard = forced.1.decode(samples, None);
        self.forced = Some(forced);
        heard
    }
}

fn recognizer(
    model: &Path,
    tokens: &Path,
    language: &str,
    threads: u32,
) -> Result<OfflineRecognizer> {
    let model = path_cstring(model, "SenseVoice model path")?;
    let tokens = path_cstring(tokens, "SenseVoice tokens path")?;
    let language = cstring(language)?;
    let provider = cstring("cpu")?;
    let decoding = cstring("greedy_search")?;
    let mut config = zeroed_config();
    config.feat_config.feature_dim = 80;
    config.model_config.sense_voice.model = model.as_ptr();
    config.model_config.sense_voice.language = language.as_ptr();
    // Inverse text normalization gives punctuation and digits, like the live text.
    config.model_config.sense_voice.use_itn = 1;
    config.model_config.tokens = tokens.as_ptr();
    config.model_config.num_threads = threads as i32;
    config.model_config.provider = provider.as_ptr();
    config.decoding_method = decoding.as_ptr();
    // Every string above lives through this call.
    OfflineRecognizer::new(NAME, &config)
}
