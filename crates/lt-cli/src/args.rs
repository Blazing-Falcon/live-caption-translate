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
    /// Read and translate the text in a PNG screenshot.
    Screen(ScreenArgs),
    /// Download and verify the pinned model files.
    Models {
        #[command(subcommand)]
        command: ModelCommands,
    },
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
    /// Speech instants for the per-word delays: a JSON array of milliseconds from `--start`.
    #[arg(long)]
    pub instants: Option<PathBuf>,
}

#[derive(Args)]
pub struct ScreenArgs {
    #[arg(long, value_enum, default_value = "horizontal")]
    pub direction: DirectionArg,
    pub png: PathBuf,
    #[arg(long, default_value = "out/screen.json")]
    pub out: PathBuf,
    #[arg(long, env = "LT_MODELS_DIR")]
    pub models: Option<PathBuf>,
    /// Override the bundled binary for local development.
    #[arg(long, env = "LT_LLAMA_SERVER")]
    pub server: Option<PathBuf>,
    /// Read and group the text only; do not start the translator.
    #[arg(long)]
    pub no_translate: bool,
    /// Include grouping decisions and line identities in the output JSON.
    #[arg(long)]
    pub explain: bool,
    /// Threads for text recognition (default: the app's choice, at most 4).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    pub threads: Option<u16>,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum DirectionArg {
    Horizontal,
    Vertical,
}

impl From<DirectionArg> for lt_core::screen::Direction {
    fn from(value: DirectionArg) -> Self {
        match value {
            DirectionArg::Horizontal => Self::Horizontal,
            DirectionArg::Vertical => Self::Vertical,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum PaceArg {
    Realtime,
    Fast,
}

#[derive(Subcommand)]
pub enum ModelCommands {
    Fetch {
        #[arg(long, env = "LT_MODELS_DIR")]
        dir: Option<PathBuf>,
        /// Also download the optional models (the screen text recognition model).
        #[arg(long)]
        optional: bool,
    },
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
