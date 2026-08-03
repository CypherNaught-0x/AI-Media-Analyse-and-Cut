//! Live CrisperWhisper tests: they drive the real bridge script, the real
//! Python environment and the real model weights against the known recording
//! in `dev-resources/test-data/`.
//!
//! These are skipped automatically unless a CrisperWhisper environment is
//! available, so `cargo test` stays green on a machine that has never set one
//! up. To run them:
//!
//! ```bash
//! # Use the environment the app manages (macOS path shown):
//! cargo test --test crisper_integration -- --nocapture
//!
//! # Or point at any interpreter that has `crisperwhisper` installed:
//! CRISPER_TEST_PYTHON=/path/to/venv/bin/python3 cargo test --test crisper_integration -- --nocapture
//! ```
//!
//! Set `CRISPER_TEST_MODEL` to pick a size (default `small`, so CI-ish runs
//! stay to a ~1 GB download rather than the ~5 GB `large` weights).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ai_media_cutter_lib::crisper::{build_segments, CrisperOptions, RunnerWord};

const RUNNER_SOURCE: &str = include_str!("../resources/crisperwhisper_runner.py");

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .to_path_buf()
}

/// Interpreters to consider, in the same spirit as the app's own resolution:
/// an explicit override first, then the environment the app manages.
fn candidate_pythons() -> Vec<PathBuf> {
    if let Ok(explicit) = std::env::var("CRISPER_TEST_PYTHON") {
        let explicit = explicit.trim().to_string();
        if !explicit.is_empty() {
            return vec![PathBuf::from(explicit)];
        }
    }

    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        // macOS
        candidates.push(
            home.join("Library/Application Support/itemis.ai-media-cutter")
                .join("python/crisperwhisper/bin/python3"),
        );
        // Linux
        candidates.push(
            home.join(".local/share/itemis.ai-media-cutter")
                .join("python/crisperwhisper/bin/python3"),
        );
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(
            PathBuf::from(appdata)
                .join("itemis.ai-media-cutter")
                .join("python/crisperwhisper/Scripts/python.exe"),
        );
    }

    candidates
}

/// Write the embedded bridge script to a temp file, exactly as the app does.
fn materialize_runner() -> PathBuf {
    let path = std::env::temp_dir().join("crisperwhisper_runner_test.py");
    std::fs::write(&path, RUNNER_SOURCE).expect("write runner script");
    path
}

#[derive(Debug)]
struct Runner {
    python: PathBuf,
    script: PathBuf,
}

impl Runner {
    /// Send a request and return the parsed protocol lines.
    fn call(&self, request: &serde_json::Value) -> (Vec<String>, Option<serde_json::Value>) {
        let mut child = Command::new(&self.python)
            .arg(&self.script)
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn python");

        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(request.to_string().as_bytes())
            .expect("write request");

        let output = child.wait_with_output().expect("wait for python");
        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut progress = Vec::new();
        let mut result = None;
        for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
            let message: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("non-protocol stdout line {line:?}: {error}"));
            match message.get("type").and_then(|value| value.as_str()) {
                Some("progress") => progress.push(
                    message
                        .get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                ),
                Some("result") | Some("error") => result = Some(message),
                _ => {}
            }
        }

        (progress, result)
    }
}

/// Find a usable environment, or `None` so the test can skip.
fn available_runner() -> Option<Runner> {
    let script = materialize_runner();

    for python in candidate_pythons() {
        if !python.exists() {
            continue;
        }
        let runner = Runner {
            python: python.clone(),
            script: script.clone(),
        };
        let (_, result) = runner.call(&serde_json::json!({ "action": "probe" }));
        let Some(result) = result else { continue };

        let ready = result
            .get("installed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            && result
                .get("pythonSupported")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            && result
                .get("backends")
                .and_then(|value| value.as_array())
                .map(|backends| !backends.is_empty())
                .unwrap_or(false);

        if ready {
            println!(
                "Using CrisperWhisper environment: {} (crisperwhisper {}, backends {})",
                python.display(),
                result
                    .get("crisperwhisperVersion")
                    .and_then(|value| value.as_str())
                    .unwrap_or("?"),
                result.get("backends").cloned().unwrap_or_default()
            );
            return Some(runner);
        }
    }

    None
}

/// Normalise the fixture recording to the 16 kHz mono WAV the app feeds in.
fn prepare_wav() -> Option<PathBuf> {
    let source = repo_root().join("dev-resources/test-data/test_podcast.m4a");
    assert!(
        source.exists(),
        "test recording missing at {}",
        source.display()
    );

    let destination = std::env::temp_dir().join("crisper_integration_16k.wav");
    if destination.exists() {
        return Some(destination);
    }

    let ffmpeg = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string());
    let status = Command::new(ffmpeg)
        .arg("-y")
        .arg("-i")
        .arg(&source)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(&destination)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(status) if status.success() && destination.exists() => Some(destination),
        _ => None,
    }
}

fn test_model() -> String {
    std::env::var("CRISPER_TEST_MODEL").unwrap_or_else(|_| "small".to_string())
}

fn parse_seconds(timestamp: &str) -> f64 {
    // "MM:SS.mmm" or "HH:MM:SS.mmm"
    let parts: Vec<&str> = timestamp.split(':').collect();
    let mut seconds = 0.0;
    for part in &parts {
        seconds = seconds * 60.0 + part.parse::<f64>().expect("numeric timestamp part");
    }
    seconds
}

#[test]
fn probe_reports_a_usable_environment_or_skips() {
    let Some(runner) = available_runner() else {
        println!(
            "Skipping: no CrisperWhisper environment found. \
             Set it up in the app, or set CRISPER_TEST_PYTHON."
        );
        return;
    };

    let (_, result) = runner.call(&serde_json::json!({ "action": "probe" }));
    let result = result.expect("probe result");

    assert_eq!(result["type"], "result");
    assert!(result["installed"].as_bool().unwrap());
    assert!(result["pythonSupported"].as_bool().unwrap());
    // Only the two documented backend names may appear.
    for backend in result["backends"].as_array().unwrap() {
        let backend = backend.as_str().unwrap();
        assert!(
            backend == "transformers" || backend == "ct2",
            "unexpected backend {backend}"
        );
    }
}

#[test]
fn rejects_languages_the_model_is_not_published_for() {
    let Some(runner) = available_runner() else {
        println!("Skipping: no CrisperWhisper environment found.");
        return;
    };

    let Some(wav) = prepare_wav() else {
        println!("Skipping: FFmpeg unavailable to prepare the test audio.");
        return;
    };

    // French is not on the model card. With otherwise-valid input this must be
    // rejected outright, and fast — before any weights are loaded.
    let started = std::time::Instant::now();
    let (_, result) = runner.call(&serde_json::json!({
        "action": "transcribe",
        "audioPath": wav.to_string_lossy(),
        "model": test_model(),
        "language": "fr",
        "mode": "verbatim",
    }));
    let result = result.expect("error result");
    assert!(
        started.elapsed().as_secs() < 30,
        "unsupported language should be rejected without loading the model"
    );

    assert_eq!(result["type"], "error");
    assert_eq!(result["kind"], "input");
    let message = result["message"].as_str().unwrap();
    assert!(
        message.contains("English and German"),
        "unhelpful message: {message}"
    );
}

/// The headline test: transcribe the known recording and check the output
/// against the hand-checked gold standard.
#[test]
fn transcribes_the_known_recording_verbatim_with_word_timings() {
    let Some(runner) = available_runner() else {
        println!("Skipping: no CrisperWhisper environment found.");
        return;
    };
    let Some(wav) = prepare_wav() else {
        println!("Skipping: FFmpeg unavailable to prepare the test audio.");
        return;
    };

    let model = test_model();
    println!("Transcribing with CrisperWhisper '{model}'...");

    let (progress, result) = runner.call(&serde_json::json!({
        "action": "transcribe",
        "audioPath": wav.to_string_lossy(),
        "model": model,
        "language": "en",
        "mode": "verbatim",
        "wordTimestamps": true,
    }));

    let result = result.expect("a result or error was emitted");
    assert_eq!(
        result["type"], "result",
        "transcription failed: {result:#?} (progress: {progress:#?})"
    );

    // --- word timings ---------------------------------------------------
    let words = result["words"].as_array().expect("words array");
    assert!(
        words.len() > 80,
        "expected a substantial word stream for 74s of speech, got {}",
        words.len()
    );

    let duration = result["duration"].as_f64().unwrap_or_default();
    assert!(
        (duration - 74.25).abs() < 1.0,
        "reported duration {duration} does not match the 74.25s recording"
    );

    let mut previous_start = -1.0;
    for word in words {
        let start = word["start"].as_f64().expect("word start");
        let end = word["end"].as_f64().expect("word end");
        assert!(end >= start, "word ends before it starts: {word:#?}");
        assert!(
            start >= previous_start - 0.001,
            "word starts went backwards at {word:#?}"
        );
        assert!(
            start >= -0.001 && end <= duration + 1.0,
            "word timing outside the audio: {word:#?}"
        );
        previous_start = start;
    }

    // --- verbatim content ----------------------------------------------
    let text = result["text"].as_str().unwrap().to_lowercase();
    // Distinctive phrases from the gold standard transcript.
    for phrase in [
        "unpack this",
        "deep dive",
        "media cutter",
        "open source",
        "redefine video editing",
    ] {
        assert!(text.contains(phrase), "transcript is missing {phrase:?}");
    }

    // Verbatim mode must surface the disfluency the gold standard records as
    // "that's uh really trying to redefine video editing".
    let fillers: Vec<&serde_json::Value> = words
        .iter()
        .filter(|word| word["filler"].as_bool().unwrap_or(false))
        .collect();
    assert!(
        !fillers.is_empty(),
        "verbatim mode found no fillers, but the recording contains at least one \"uh\""
    );
    for filler in &fillers {
        let token = filler["text"].as_str().unwrap().to_lowercase();
        assert!(
            token == "[um]" || token == "[uh]",
            "unexpected filler token {token}"
        );
    }
    println!(
        "Found {} filler(s): {:?}",
        fillers.len(),
        fillers
            .iter()
            .map(|f| (
                f["text"].as_str().unwrap(),
                f["start"].as_f64().unwrap(),
                f["end"].as_f64().unwrap()
            ))
            .collect::<Vec<_>>()
    );
}

/// Removing fillers must cut their audio, not just their text: the resulting
/// segments have to leave a gap where each filler was, because the video export
/// concatenates segment spans.
#[test]
fn removing_fillers_leaves_a_gap_the_export_can_cut() {
    let Some(runner) = available_runner() else {
        println!("Skipping: no CrisperWhisper environment found.");
        return;
    };
    let Some(wav) = prepare_wav() else {
        println!("Skipping: FFmpeg unavailable to prepare the test audio.");
        return;
    };

    let (_, result) = runner.call(&serde_json::json!({
        "action": "transcribe",
        "audioPath": wav.to_string_lossy(),
        "model": test_model(),
        "language": "en",
        "mode": "verbatim",
        "wordTimestamps": true,
    }));
    let result = result.expect("result");
    assert_eq!(result["type"], "result", "transcription failed: {result:#?}");

    let words: Vec<RunnerWord> =
        serde_json::from_value(result["words"].clone()).expect("words deserialize");
    let filler_spans: Vec<(f32, f32)> = words
        .iter()
        .filter(|word| word.filler)
        .map(|word| (word.start, word.end))
        .collect();
    assert!(
        !filler_spans.is_empty(),
        "no fillers in the recording to remove"
    );

    // Same mapping the Tauri command uses, with and without filler removal.
    let kept = build_segments(&words, &[], &CrisperOptions::default());
    let stripped = build_segments(
        &words,
        &[],
        &CrisperOptions {
            remove_fillers: true,
            ..Default::default()
        },
    );

    assert!(
        kept.iter().any(|segment| segment.text.contains("[UH]")
            || segment.text.contains("[UM]")),
        "fillers should be present when they are not removed"
    );
    assert!(
        stripped
            .iter()
            .all(|segment| !segment.text.contains("[UH]") && !segment.text.contains("[UM]")),
        "fillers still present after removal"
    );

    // Every filler span must fall outside all kept segments.
    for (filler_start, filler_end) in &filler_spans {
        let midpoint = ((filler_start + filler_end) / 2.0) as f64;
        let covered = stripped.iter().any(|segment| {
            let start = parse_seconds(&segment.start);
            let end = parse_seconds(&segment.end);
            midpoint > start && midpoint < end
        });
        assert!(
            !covered,
            "filler at {filler_start}-{filler_end}s is still inside a segment span, \
             so the export would keep its audio"
        );
    }

    println!(
        "{} segments with fillers, {} after removal; {} filler span(s) excised",
        kept.len(),
        stripped.len(),
        filler_spans.len()
    );
}

/// Intended mode should return the cleaned-up reading, without filler tokens.
#[test]
fn intended_mode_returns_a_clean_transcript() {
    let Some(runner) = available_runner() else {
        println!("Skipping: no CrisperWhisper environment found.");
        return;
    };
    let Some(wav) = prepare_wav() else {
        println!("Skipping: FFmpeg unavailable to prepare the test audio.");
        return;
    };

    let (_, result) = runner.call(&serde_json::json!({
        "action": "transcribe",
        "audioPath": wav.to_string_lossy(),
        "model": test_model(),
        "language": "en",
        "mode": "intended",
        "wordTimestamps": true,
    }));
    let result = result.expect("result");
    assert_eq!(result["type"], "result", "transcription failed: {result:#?}");

    let text = result["text"].as_str().unwrap();
    assert!(!text.trim().is_empty(), "intended mode returned no text");
    assert!(
        !text.to_lowercase().contains("[uh]") && !text.to_lowercase().contains("[um]"),
        "intended mode should not emit filler tokens, got: {text}"
    );
    assert_eq!(result["mode"], "intended");
}

/// Guard the runner's stdout contract: only protocol JSON reaches stdout, even
/// though the ML stack prints plenty of its own noise.
#[test]
fn runner_stdout_carries_only_protocol_json() {
    let Some(runner) = available_runner() else {
        println!("Skipping: no CrisperWhisper environment found.");
        return;
    };

    // `call` panics on any non-JSON stdout line, so a clean probe proves it.
    let (_, result) = runner.call(&serde_json::json!({ "action": "probe" }));
    assert!(result.is_some());

    let script = Path::new(&runner.script);
    assert!(script.exists());
}
