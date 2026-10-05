use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Offline Chinese-to-English live captions and headless replay"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Replay a WAV through the full caption pipeline.
    Replay(ReplayArgs),
    /// Download and verify the pinned model files.
    Models {
        #[command(subcommand)]
        command: ModelCommands,
    },
    /// Measure a real engine on local WAV clips or translation fixtures.
    Bench(BenchArgs),
    /// Capture Windows playback without opening a window.
    Live(LiveArgs),
}

#[derive(Args)]
pub struct ReplayArgs {
    pub wav: PathBuf,
    #[arg(long, default_value_t = 0.0, value_parser = nonnegative)]
    pub start: f64,
    #[arg(long, value_parser = positive)]
    pub dur: Option<f64>,
    #[arg(long, value_enum, default_value = "realtime")]
    pub pace: PaceArg,
    #[arg(long, default_value = "out/replay.jsonl")]
    pub out: PathBuf,
    #[arg(long, env = "LT_CONFIG")]
    pub config: Option<PathBuf>,
    #[arg(long, env = "LT_MODELS_DIR")]
    pub models: Option<PathBuf>,
    /// Override the bundled binary for local development.
    #[arg(long, env = "LT_LLAMA_SERVER")]
    pub server: Option<PathBuf>,
    /// Use fake engines for terminal-only pipeline diagnostics.
    #[arg(long)]
    pub fake: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum PaceArg {
    Realtime,
    Fast,
}

#[derive(Subcommand)]
pub enum ModelCommands {
    Fetch {
        #[arg(long, value_enum, default_value = "hf")]
        source: DownloadSource,
        #[arg(long, env = "LT_MODELS_DIR")]
        dir: Option<PathBuf>,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum DownloadSource {
    Hf,
    Modelscope,
}

#[derive(Args)]
pub struct BenchArgs {
    #[arg(value_enum)]
    pub engine: BenchEngine,
    /// WAV file/folder for ASR/VAD, or sentence JSON for MT.
    #[arg(default_value = "testdata/fetched/ascend")]
    pub input: PathBuf,
    #[arg(long, env = "LT_MODELS_DIR")]
    pub models: Option<PathBuf>,
    #[arg(long, env = "LT_CONFIG")]
    pub config: Option<PathBuf>,
    #[arg(long, env = "LT_LLAMA_SERVER")]
    pub server: Option<PathBuf>,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum BenchEngine {
    Asr,
    Mt,
    Vad,
}

#[derive(Args)]
pub struct LiveArgs {
    #[arg(long, value_enum, default_value = "system")]
    pub mode: CaptureModeArg,
    #[arg(long, action = clap::ArgAction::Append)]
    pub app: Vec<String>,
    #[arg(long, env = "LT_CONFIG")]
    pub config: Option<PathBuf>,
    #[arg(long, env = "LT_MODELS_DIR")]
    pub models: Option<PathBuf>,
    #[arg(long, env = "LT_LLAMA_SERVER")]
    pub server: Option<PathBuf>,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum CaptureModeArg {
    System,
    Apps,
}

fn nonnegative(value: &str) -> Result<f64, String> {
    let number = value
        .parse::<f64>()
        .map_err(|_| "Expected seconds".to_owned())?;
    if !number.is_finite() || number < 0.0 {
        return Err("Seconds must be finite and nonnegative".into());
    }
    Ok(number)
}
fn positive(value: &str) -> Result<f64, String> {
    let number = nonnegative(value)?;
    if number == 0.0 {
        return Err("Duration must be positive".into());
    }
    Ok(number)
}
