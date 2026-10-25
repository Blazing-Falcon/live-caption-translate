mod args;
mod engines;
mod replay;
mod report;
mod wordreport;

#[cfg(not(feature = "llm"))]
use anyhow::bail;
use anyhow::{Context, Result};
use args::{Cli, Commands, ModelCommands};
use clap::Parser;
use lt_core::config::{Config, MessageLevel};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = match &cli.command {
        Commands::Replay(args) => args.config.as_deref(),
        Commands::Models { .. } => None,
    };
    let (config, messages) = engines::config(config_path)?;
    let filter = std::env::var("LT_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_else(|_| config.logging.level.to_string());
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(filter).context("Invalid logging filter")?,
        )
        .init();
    for message in &messages {
        if message.level == MessageLevel::Warn {
            tracing::warn!(message = %message.text, "Config");
        } else {
            tracing::info!(message = %message.text, "Config");
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal_flag = cancelled.clone();
    ctrlc::set_handler(move || signal_flag.store(true, Ordering::Release))
        .context("Installing graceful shutdown signal handler")?;
    let result = match cli.command {
        Commands::Replay(args) => replay::replay(args, config, cancelled.clone()),
        Commands::Models { command } => models(command, config, cancelled.clone()),
    };
    match result {
        Err(error)
            if cancelled.load(Ordering::Acquire)
                && matches!(
                    error.downcast_ref::<lt_core::error::Error>(),
                    Some(lt_core::error::Error::Stopped)
                ) =>
        {
            Ok(())
        }
        result => result,
    }
}

#[cfg(feature = "llm")]
fn models(command: ModelCommands, config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    match command {
        ModelCommands::Fetch { dir } => {
            let dir = engines::models_dir(dir.as_deref(), &config);
            let statuses = lt_llm::models::fetch(dir, &cancelled, &mut |status| {
                eprintln!(
                    "{} {:?}: {}/{} bytes",
                    status.id, status.state, status.bytes_done, status.bytes_total
                );
            })?;
            println!("{}", serde_json::to_string(&statuses)?);
            Ok(())
        }
    }
}
#[cfg(not(feature = "llm"))]
fn models(_: ModelCommands, _: Config, _: Arc<AtomicBool>) -> Result<()> {
    bail!("Model download requires --features llm");
}
