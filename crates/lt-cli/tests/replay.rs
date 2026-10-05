//! Invokes the real headless CLI; no sound device or visible window is used.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let path = workspace.join(format!(
            "target/tmp/cli-replay-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let path = self.0.canonicalize().unwrap();
        assert!(path.starts_with(workspace.join("target/tmp")));
        fs::remove_dir_all(path).unwrap();
    }
}

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lt-cli"));
    command.current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

#[test]
fn fast_replay_writes_the_shared_utf8_records_and_complete_summary() {
    let scratch = Scratch::new();
    let wav = scratch.0.join("speech.wav");
    let config = scratch.0.join("config.toml");
    let out = scratch.0.join("replay.jsonl");
    fs::write(
        &config,
        "[join]\nhold_max_chars=0\n[audio]\nnormalize=false\n",
    )
    .unwrap();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&wav, spec).unwrap();
    for at in 0..132_300 {
        let sample = if (22_050..88_200).contains(&at) {
            (0.3 * (std::f64::consts::TAU * 1000.0 * at as f64 / 44_100.0).sin() * 32767.0) as i16
        } else {
            0
        };
        writer.write_sample(sample).unwrap();
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
    let output = cli()
        .arg("replay")
        .arg(&wav)
        .args(["--fake", "--pace", "fast", "--config"])
        .arg(&config)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<Value> = fs::read_to_string(&out)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(records[0]["type"], "session");
    assert_eq!(records[0]["asr"], "fake");
    assert_eq!(records[1]["type"], "utterance");
    assert_eq!(records[1]["source"], "你好，这是测试语音。");
    assert_eq!(records[1]["english"], "Hello.");
    assert_eq!(records[1]["status"], "final");
    assert_eq!(records[2]["segments"], 1);
    assert_eq!(records[2]["cuts"]["pause"], 1);
    assert_eq!(records[2]["status"]["final"], 1);
    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout, records[2]);
}

#[test]
fn invalid_replay_range_and_missing_wav_exit_with_readable_errors() {
    let output = cli()
        .args(["replay", "missing.wav", "--fake", "--start", "NaN"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("finite"));
    let output = cli()
        .args(["replay", "missing.wav", "--fake"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("WAV"));
}

#[test]
fn environment_config_is_loaded_and_explicit_config_takes_precedence() {
    let scratch = Scratch::new();
    let config = scratch.0.join("environment.toml");
    let override_config = scratch.0.join("override.toml");
    let wav = scratch.0.join("silent.wav");
    let out = scratch.0.join("environment.jsonl");
    fs::write(
        &config,
        "[vad]\nmin_silence_s=0.75\n[logging]\nlevel=\"debug\"\n",
    )
    .unwrap();
    fs::write(&override_config, "[vad]\nmin_silence_s=0.6\n").unwrap();
    let mut writer = hound::WavWriter::create(
        &wav,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..512 {
        writer.write_sample(0_i16).unwrap();
    }
    writer.finalize().unwrap();
    for explicit in [false, true] {
        let mut command = cli();
        command
            .env("LT_CONFIG", &config)
            .env("LT_LOG", "warn,lt_core=error")
            .env("LT_MODELS_DIR", scratch.0.join("unused-models"))
            .arg("replay")
            .arg(&wav)
            .args(["--fake", "--pace", "fast", "--out"])
            .arg(&out);
        if explicit {
            command.arg("--config").arg(&override_config);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let contents = fs::read_to_string(&out).unwrap();
        let header: Value = serde_json::from_str(contents.lines().next().unwrap()).unwrap();
        assert_eq!(
            header["config"]["min_silence_s"],
            if explicit { 0.6 } else { 0.75 }
        );
    }
    let output = cli()
        .env("LT_CONFIG", &config)
        .env("LT_LOG", "[invalid")
        .arg("replay")
        .arg(&wav)
        .args(["--fake", "--pace", "fast"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid logging filter"));
}
