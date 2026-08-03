//! CrisperWhisper 2.0 transcription backend.
//!
//! CrisperWhisper is a verbatim-first Whisper derivative: it writes down what
//! was actually said (fillers, repetitions, stutters, vocal events) and carries
//! word timings accurate to a few tens of milliseconds. That combination is
//! what makes it useful here — knowing exactly *where* an "um" is lets the
//! editor cut it out of the video, not just out of the text.
//!
//! The model is published only as PyTorch (`safetensors`) and CTranslate2
//! weights; there is no ONNX or GGML export, so it cannot be run through the
//! `ort` runtime the Parakeet backend uses. It is therefore driven
//! out-of-process via the official `crisperwhisper` Python package, which the
//! app installs into a self-managed virtual environment on request. The
//! `transformers` extra runs anywhere PyTorch does (macOS, Windows, Linux, CPU
//! or GPU), which is what keeps this cross-platform; the faster `ct2` extra is
//! Linux x86_64 + NVIDIA only and is used automatically when present.
//!
//! Licensing note: the published weights are under a **non-commercial research
//! license**, and the model card declares **English and German** only. Both
//! facts are surfaced in the UI rather than buried here.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::Manager;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use parakeet_rs::sortformer::SpeakerSegment;

use crate::local_asr::{
    build_transcript_segments_from_runs, diarize, emit_progress, load_audio_16k_mono,
    resolve_sortformer_file, speaker_label_for_word, write_wav_16k_mono, WordWithSpeaker,
};
use crate::video::TranscriptSegment;

/// The bridge script is embedded in the binary rather than shipped as a Tauri
/// resource: it removes any chance of the script and the binary disagreeing,
/// and avoids per-platform resource-path handling.
const RUNNER_SOURCE: &str = include_str!("../resources/crisperwhisper_runner.py");
const RUNNER_FILE_NAME: &str = "crisperwhisper_runner.py";
const ENVIRONMENT_DIR_NAME: &str = "crisperwhisper";
const DEFAULT_MODEL: &str = "large";
const DEFAULT_LANGUAGE: &str = "en";
const DEFAULT_MODE: &str = "verbatim";

/// Languages the CrisperWhisper 2.0 model card is published for.
pub(crate) const SUPPORTED_LANGUAGES: [&str; 2] = ["en", "de"];

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrisperOptions {
    /// Explicit interpreter to use. Empty means: managed environment, then a
    /// system Python.
    pub python_path: String,
    /// Size shorthand (`large`/`turbo`/`medium`/`small`), a HuggingFace id, or
    /// a local model directory.
    pub model: String,
    pub language: String,
    /// `verbatim` (what was said) or `intended` (cleaned up).
    pub mode: String,
    /// `auto` | `ct2` | `transformers`
    pub backend: String,
    /// `auto` | `cpu` | `cuda`
    pub device: String,
    /// `auto` | `float32` | `float16` | `int8_float16`
    pub compute_type: String,
    #[serde(default = "default_true")]
    pub word_timestamps: bool,
    /// Drop `[UM]`/`[UH]` and split the segment there so the cut excises them.
    pub remove_fillers: bool,
    /// Same treatment for `[laughter]`, `[breath]`, `[cough]`, ...
    pub remove_vocal_events: bool,
    /// Assign speakers with Sortformer (CrisperWhisper does not diarize).
    pub diarize: bool,
    pub sortformer_model_path: String,
    /// Honoured by Pro models only; standard weights warn and ignore it.
    pub hotwords: Vec<String>,
}

impl CrisperOptions {
    fn model_or_default(&self) -> &str {
        let trimmed = self.model.trim();
        if trimmed.is_empty() {
            DEFAULT_MODEL
        } else {
            trimmed
        }
    }

    fn language_or_default(&self) -> &str {
        let trimmed = self.language.trim();
        if trimmed.is_empty() {
            DEFAULT_LANGUAGE
        } else {
            trimmed
        }
    }

    fn mode_or_default(&self) -> &str {
        let trimmed = self.mode.trim();
        if trimmed.is_empty() {
            DEFAULT_MODE
        } else {
            trimmed
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrisperEnvironmentStatus {
    /// Interpreter that was probed, if one could be found at all.
    pub python_path: String,
    pub python: String,
    pub python_supported: bool,
    pub minimum_python: String,
    /// Whether the `crisperwhisper` package imports successfully.
    pub installed: bool,
    pub crisperwhisper_version: Option<String>,
    /// Installed inference backends: `transformers` and/or `ct2`.
    pub backends: Vec<String>,
    pub torch_version: Option<String>,
    pub cuda: bool,
    pub mps: bool,
    /// Path of the app-managed virtual environment (whether or not it exists).
    pub environment_dir: String,
    pub managed_environment_exists: bool,
    /// True when a transcription can actually be started.
    pub ready: bool,
    /// Human-readable reason when `ready` is false.
    pub message: Option<String>,
}

/// One timed word as reported by the bridge script.
///
/// Public so integration tests can feed real captured model output straight
/// into [`build_segments`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerWord {
    pub text: String,
    pub start: f32,
    pub end: f32,
    /// `[UM]` / `[UH]`.
    #[serde(default)]
    pub filler: bool,
    /// `[laughter]`, `[breath]`, `[cough]`, ...
    #[serde(default)]
    pub vocal_event: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerResult {
    #[serde(default)]
    text: String,
    #[serde(default)]
    words: Vec<RunnerWord>,
    #[serde(default)]
    backend: String,
    #[serde(default)]
    device: String,
    #[serde(default)]
    compute_type: String,
}

/// Path of the app-managed virtual environment.
fn environment_dir(window: &tauri::Window) -> Result<PathBuf> {
    window
        .path()
        .app_data_dir()
        .map_err(|error| anyhow!(error.to_string()))
        .map(|path| path.join("python").join(ENVIRONMENT_DIR_NAME))
}

/// Interpreter inside a virtual environment, per platform layout.
fn venv_python(environment: &Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        environment.join("Scripts").join("python.exe")
    } else {
        environment.join("bin").join("python3")
    }
}

/// System interpreters to try, newest first.
///
/// Plain `python3` is listed but cannot be relied on: on macOS it is the 3.9
/// command-line-tools build, which is below CrisperWhisper's 3.10 floor. The
/// versioned names are what actually find a Homebrew/python.org install, so
/// auto-setup works without the user hand-picking an interpreter.
const VERSIONED_PYTHONS: [&str; 6] = [
    "python3.14",
    "python3.13",
    "python3.12",
    "python3.11",
    "python3.10",
    "python3",
];

fn system_python_candidates() -> Vec<String> {
    if cfg!(target_os = "windows") {
        // The `py` launcher resolves the newest installed version.
        let mut candidates = vec!["py".to_string()];
        candidates.extend(
            VERSIONED_PYTHONS
                .iter()
                .map(|name| format!("{name}.exe")),
        );
        candidates.push("python.exe".to_string());
        candidates
    } else {
        let mut candidates: Vec<String> =
            VERSIONED_PYTHONS.iter().map(|name| name.to_string()).collect();
        candidates.push("python".to_string());
        candidates
    }
}

/// Candidate interpreters in priority order: explicit setting, then the
/// managed environment, then whatever the system offers.
fn python_candidates(window: &tauri::Window, python_path: &str) -> Vec<String> {
    let trimmed = python_path.trim();
    if !trimmed.is_empty() {
        return vec![trimmed.to_string()];
    }

    let mut candidates = Vec::new();
    if let Ok(environment) = environment_dir(window) {
        let managed = venv_python(&environment);
        if managed.exists() {
            candidates.push(managed.to_string_lossy().to_string());
        }
    }

    candidates.extend(system_python_candidates());
    candidates
}

/// Materialise the embedded bridge script next to the managed environment so
/// the interpreter has a real file to execute.
fn write_runner_script(window: &tauri::Window) -> Result<PathBuf> {
    let directory = window
        .path()
        .app_data_dir()
        .map_err(|error| anyhow!(error.to_string()))?
        .join("python");
    std::fs::create_dir_all(&directory).with_context(|| {
        format!(
            "Failed to create script directory '{}'",
            directory.display()
        )
    })?;

    let script_path = directory.join(RUNNER_FILE_NAME);
    // Rewrite only when the content differs so the file is not churned on
    // every run.
    let needs_write = std::fs::read_to_string(&script_path)
        .map(|existing| existing != RUNNER_SOURCE)
        .unwrap_or(true);
    if needs_write {
        std::fs::write(&script_path, RUNNER_SOURCE).with_context(|| {
            format!("Failed to write runner script '{}'", script_path.display())
        })?;
    }

    Ok(script_path)
}

fn command_for(python: &str, script: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(python);
    command
        .arg(script)
        // Unbuffered so progress lines arrive while the model is running.
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Run the bridge script with a JSON request, forwarding `progress` lines to
/// the UI and returning the final `result` object.
async fn run_runner(
    window: &tauri::Window,
    python: &str,
    script: &Path,
    request: serde_json::Value,
) -> Result<serde_json::Value> {
    let mut child = command_for(python, script)
        .spawn()
        .with_context(|| format!("Failed to start Python interpreter '{python}'"))?;

    let payload = serde_json::to_vec(&request)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&payload).await?;
        stdin.shutdown().await?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Failed to capture Python stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Failed to capture Python stderr"))?;

    // Drain stderr concurrently: pip and torch are chatty, and a full pipe
    // buffer would deadlock the child.
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            log::debug!("crisperwhisper: {line}");
            tail.push(line);
            if tail.len() > 40 {
                tail.remove(0);
            }
        }
        tail
    });

    let mut result: Option<serde_json::Value> = None;
    let mut failure: Option<String> = None;
    let mut lines = BufReader::new(stdout).lines();

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(message) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            log::debug!("crisperwhisper (non-protocol stdout): {trimmed}");
            continue;
        };

        match message.get("type").and_then(|value| value.as_str()) {
            Some("progress") => {
                if let Some(text) = message.get("message").and_then(|value| value.as_str()) {
                    emit_progress(window, text)?;
                }
            }
            Some("error") => {
                let text = message
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("CrisperWhisper failed");
                let detail = message.get("detail").and_then(|value| value.as_str());
                failure = Some(match detail {
                    Some(detail) => format!("{text} ({detail})"),
                    None => text.to_string(),
                });
            }
            Some("result") => result = Some(message),
            _ => {}
        }
    }

    let status = child.wait().await?;
    let stderr_tail = stderr_task.await.unwrap_or_default();

    if let Some(failure) = failure {
        return Err(anyhow!(failure));
    }

    match result {
        Some(result) => Ok(result),
        None => {
            let tail = stderr_tail.join("\n");
            Err(anyhow!(
                "CrisperWhisper produced no result (exit {}).{}",
                status.code().unwrap_or(-1),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(" Details: {tail}")
                }
            ))
        }
    }
}

/// Probe one interpreter. Returns `Err` only when the interpreter itself could
/// not be run; a working interpreter without the package reports
/// `installed: false`.
async fn probe_python(
    window: &tauri::Window,
    python: &str,
    script: &Path,
) -> Result<CrisperEnvironmentStatus> {
    let response = run_runner(
        window,
        python,
        script,
        serde_json::json!({ "action": "probe" }),
    )
    .await?;

    let mut status: CrisperEnvironmentStatus = serde_json::from_value(response)
        .context("Failed to parse CrisperWhisper environment probe")?;

    // The probe reports the interpreter it actually ran as; keep it.
    if status.python_path.trim().is_empty() {
        status.python_path = python.to_string();
    }

    Ok(status)
}

fn finalize_status(mut status: CrisperEnvironmentStatus) -> CrisperEnvironmentStatus {
    if !status.python_supported {
        status.ready = false;
        status.message = Some(format!(
            "Python {} found, but CrisperWhisper needs Python {} or newer.",
            status.python, status.minimum_python
        ));
    } else if !status.installed {
        status.ready = false;
        status.message =
            Some("The 'crisperwhisper' package is not installed yet.".to_string());
    } else if status.backends.is_empty() {
        status.ready = false;
        status.message = Some(
            "'crisperwhisper' is installed but no inference backend is. \
             Install the PyTorch backend."
                .to_string(),
        );
    } else {
        status.ready = true;
        status.message = None;
    }

    status
}

/// Report whether CrisperWhisper can run, and on what.
#[tauri::command]
pub async fn crisper_environment_status(
    window: tauri::Window,
    python_path: String,
) -> Result<CrisperEnvironmentStatus, String> {
    let script = write_runner_script(&window).map_err(|error| error.to_string())?;
    let environment = environment_dir(&window).map_err(|error| error.to_string())?;
    let managed_exists = venv_python(&environment).exists();

    let mut last_error: Option<String> = None;
    // Remember the most informative unusable result, so a "Python 3.9 is too
    // old" message survives instead of being replaced by "python not found".
    let mut fallback: Option<CrisperEnvironmentStatus> = None;

    for candidate in python_candidates(&window, &python_path) {
        match probe_python(&window, &candidate, &script).await {
            Ok(mut status) => {
                status.environment_dir = environment.to_string_lossy().to_string();
                status.managed_environment_exists = managed_exists;
                let status = finalize_status(status);
                // A usable interpreter wins immediately; otherwise keep looking
                // (e.g. system python3 is 3.9 but the managed venv is 3.12).
                if status.ready {
                    return Ok(status);
                }
                last_error = status.message.clone().or(last_error);
                if fallback.is_none() {
                    fallback = Some(status);
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }

    if let Some(fallback) = fallback {
        return Ok(fallback);
    }

    Ok(CrisperEnvironmentStatus {
        python_path: python_path.trim().to_string(),
        environment_dir: environment.to_string_lossy().to_string(),
        managed_environment_exists: managed_exists,
        minimum_python: "3.10".to_string(),
        ready: false,
        message: Some(last_error.unwrap_or_else(|| {
            "No Python 3.10+ interpreter was found. Install Python, then set up \
             the CrisperWhisper environment."
                .to_string()
        })),
        ..Default::default()
    })
}

/// Stream a child process's merged output to the UI as progress lines.
async fn run_install_step(
    window: &tauri::Window,
    label: &str,
    program: &str,
    arguments: &[String],
) -> Result<()> {
    emit_progress(window, label)?;

    let mut child = tokio::process::Command::new(program)
        .args(arguments)
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to run '{program}'"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let window_for_stdout = window.clone();

    let stdout_task = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim().to_string();
                // pip's per-file noise is not useful; surface the milestones.
                if trimmed.starts_with("Collecting")
                    || trimmed.starts_with("Downloading")
                    || trimmed.starts_with("Installing")
                    || trimmed.starts_with("Successfully")
                {
                    let _ = emit_progress(&window_for_stdout, &trimmed);
                }
                log::debug!("crisperwhisper install: {trimmed}");
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut tail: Vec<String> = Vec::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log::debug!("crisperwhisper install: {line}");
                tail.push(line);
                if tail.len() > 40 {
                    tail.remove(0);
                }
            }
        }
        tail
    });

    let status = child.wait().await?;
    let _ = stdout_task.await;
    let tail = stderr_task.await.unwrap_or_default();

    if !status.success() {
        return Err(anyhow!(
            "{label} failed (exit {}).{}",
            status.code().unwrap_or(-1),
            if tail.is_empty() {
                String::new()
            } else {
                format!(" Details: {}", tail.join("\n"))
            }
        ));
    }

    Ok(())
}

/// Pick the pip extra to install.
///
/// `ct2` wheels are Linux x86_64 only, so everywhere else the portable PyTorch
/// backend is the only option — which is what makes this cross-platform.
fn resolve_extra(requested: &str) -> &'static str {
    let ct2_supported = cfg!(all(target_os = "linux", target_arch = "x86_64"));

    match requested.trim() {
        "ct2" if ct2_supported => "ct2",
        "all" if ct2_supported => "all",
        _ => "transformers",
    }
}

/// Create the managed virtual environment and install `crisperwhisper` into it.
#[tauri::command]
pub async fn install_crisper_environment(
    window: tauri::Window,
    python_path: String,
    extra: String,
) -> Result<CrisperEnvironmentStatus, String> {
    let run = async {
        let script = write_runner_script(&window)?;
        let environment = environment_dir(&window)?;
        let managed_python = venv_python(&environment);

        // A base interpreter is needed only to build the venv; once it exists,
        // reuse it so we never rebuild on top of an unsupported Python.
        if !managed_python.exists() {
            let mut base: Option<String> = None;
            let mut last_error: Option<String> = None;

            // Deliberately not the managed venv: it is what we are building.
            let candidates = if python_path.trim().is_empty() {
                system_python_candidates()
            } else {
                vec![python_path.trim().to_string()]
            };

            for candidate in candidates {
                match probe_python(&window, &candidate, &script).await {
                    Ok(status) if status.python_supported => {
                        base = Some(candidate);
                        break;
                    }
                    Ok(status) => {
                        last_error = Some(format!(
                            "Python {} at '{}' is too old; {} or newer is required.",
                            status.python, candidate, status.minimum_python
                        ));
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }

            let base = base.ok_or_else(|| {
                anyhow!(
                    "{}",
                    last_error.unwrap_or_else(|| {
                        "No Python 3.10+ interpreter was found. Install Python 3.10 \
                         or newer and try again."
                            .to_string()
                    })
                )
            })?;

            if let Some(parent) = environment.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            run_install_step(
                &window,
                "Creating the CrisperWhisper Python environment...",
                &base,
                &[
                    "-m".to_string(),
                    "venv".to_string(),
                    environment.to_string_lossy().to_string(),
                ],
            )
            .await?;
        }

        if !managed_python.exists() {
            return Err(anyhow!(
                "The Python environment was created but no interpreter was found at '{}'.",
                managed_python.display()
            ));
        }

        let managed_python = managed_python.to_string_lossy().to_string();
        let extra = resolve_extra(&extra);

        run_install_step(
            &window,
            "Updating pip...",
            &managed_python,
            &[
                "-m".to_string(),
                "pip".to_string(),
                "install".to_string(),
                "--upgrade".to_string(),
                "pip".to_string(),
            ],
        )
        .await?;

        run_install_step(
            &window,
            &format!(
                "Installing crisperwhisper[{extra}] — this downloads PyTorch and \
                 can take several minutes..."
            ),
            &managed_python,
            &[
                "-m".to_string(),
                "pip".to_string(),
                "install".to_string(),
                format!("crisperwhisper[{extra}]"),
            ],
        )
        .await?;

        emit_progress(&window, "Verifying the CrisperWhisper environment...")?;

        let mut status = probe_python(&window, &managed_python, &script).await?;
        status.environment_dir = environment.to_string_lossy().to_string();
        status.managed_environment_exists = true;

        Ok::<CrisperEnvironmentStatus, anyhow::Error>(finalize_status(status))
    };

    run.await.map_err(|error| error.to_string())
}

/// Choose the interpreter to transcribe with, preferring one that is ready.
async fn resolve_ready_python(
    window: &tauri::Window,
    python_path: &str,
    script: &Path,
) -> Result<String> {
    let mut last_message: Option<String> = None;

    for candidate in python_candidates(window, python_path) {
        match probe_python(window, &candidate, script).await {
            Ok(status) => {
                let status = finalize_status(status);
                if status.ready {
                    return Ok(candidate);
                }
                last_message = status.message.or(last_message);
            }
            Err(error) => last_message = Some(error.to_string()),
        }
    }

    Err(anyhow!(
        "{} Set up the CrisperWhisper environment in Settings.",
        last_message.unwrap_or_else(|| "No usable Python interpreter was found.".to_string())
    ))
}

/// Split the word stream into runs, dropping unwanted tokens and breaking the
/// segment where one was removed.
///
/// The break is what makes removal real: the export concatenates segment spans,
/// so a dropped word's time range only disappears from the video if it falls
/// between two segments.
fn partition_words(
    words: Vec<WordWithSpeaker>,
    drop: &[bool],
) -> Vec<Vec<WordWithSpeaker>> {
    let mut runs: Vec<Vec<WordWithSpeaker>> = Vec::new();
    let mut current: Vec<WordWithSpeaker> = Vec::new();

    for (word, dropped) in words.into_iter().zip(drop.iter().copied()) {
        if dropped {
            if !current.is_empty() {
                runs.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(word);
    }

    if !current.is_empty() {
        runs.push(current);
    }

    runs
}

/// Transcribe with CrisperWhisper 2.0 and return editor-ready segments.
#[tauri::command]
pub async fn transcribe_with_crisper(
    window: tauri::Window,
    audio_path: String,
    options: CrisperOptions,
) -> Result<Vec<TranscriptSegment>, String> {
    let run = async {
        let audio_file = PathBuf::from(&audio_path);
        if !audio_file.exists() {
            return Err(anyhow!("Audio file not found: {}", audio_file.display()));
        }

        let language = options.language_or_default().to_lowercase();
        if !SUPPORTED_LANGUAGES.contains(&language.as_str()) {
            return Err(anyhow!(
                "CrisperWhisper 2.0 is published for English and German only \
                 (got '{language}')."
            ));
        }

        let mode = options.mode_or_default().to_lowercase();
        if mode != "verbatim" && mode != "intended" {
            return Err(anyhow!(
                "Unsupported CrisperWhisper mode '{mode}'; expected 'verbatim' or 'intended'."
            ));
        }

        let script = write_runner_script(&window)?;
        emit_progress(&window, "Checking the CrisperWhisper environment...")?;
        let python = resolve_ready_python(&window, &options.python_path, &script).await?;

        // Diarization needs the samples; the bridge script needs a file. One
        // FFmpeg pass produces the 16 kHz mono WAV both can use.
        emit_progress(&window, "Preparing audio for CrisperWhisper...")?;
        let wav_path = std::env::temp_dir()
            .join(format!("ai-media-cutter-crisper-{}.wav", fastrand::u64(..)));
        write_wav_16k_mono(&audio_file, &wav_path)?;

        let outcome = transcribe_and_build(
            &window,
            &python,
            &script,
            &wav_path,
            &language,
            &mode,
            &options,
        )
        .await;

        let _ = std::fs::remove_file(&wav_path);
        outcome
    };

    run.await.map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn transcribe_and_build(
    window: &tauri::Window,
    python: &str,
    script: &Path,
    wav_path: &Path,
    language: &str,
    mode: &str,
    options: &CrisperOptions,
) -> Result<Vec<TranscriptSegment>> {
    // Resolve the diarization model before the long transcription so a missing
    // download fails fast rather than after minutes of inference.
    let sortformer_file = if options.diarize {
        Some(resolve_sortformer_file(window, &options.sortformer_model_path).await?)
    } else {
        None
    };

    let request = serde_json::json!({
        "action": "transcribe",
        "audioPath": wav_path.to_string_lossy(),
        "model": options.model_or_default(),
        "language": language,
        "mode": mode,
        "backend": options.backend,
        "device": options.device,
        "computeType": options.compute_type,
        "wordTimestamps": options.word_timestamps,
        "hotwords": options.hotwords,
    });

    let response = run_runner(window, python, script, request).await?;
    let result: RunnerResult =
        serde_json::from_value(response).context("Failed to parse the CrisperWhisper result")?;

    if !result.backend.is_empty() {
        log::info!(
            "CrisperWhisper ran on {} ({}, {})",
            result.backend,
            result.device,
            result.compute_type
        );
    }

    if result.words.is_empty() {
        // Without word timings there is nothing to build a timed transcript
        // from; surface it rather than returning an empty transcript silently.
        if result.text.trim().is_empty() {
            return Ok(Vec::new());
        }
        return Err(anyhow!(
            "CrisperWhisper returned a transcript without word timings, which \
             this editor needs. Enable word timings and try again."
        ));
    }

    let diarization = match sortformer_file {
        Some(file) => {
            emit_progress(window, "Running Sortformer diarization...")?;
            let audio = load_audio_16k_mono(wav_path)?;
            // Sortformer is CPU-bound; keep it off the async runtime threads.
            tokio::task::spawn_blocking(move || diarize(&file, audio))
                .await
                .map_err(|error| anyhow!("Diarization task failed: {error}"))??
        }
        None => Vec::new(),
    };

    emit_progress(window, "Building the CrisperWhisper transcript...")?;

    Ok(build_segments(&result.words, &diarization, options))
}

/// Turn the model's timed word stream into editor-ready segments.
///
/// Pure so it can be tested against real captured model output without a
/// running Tauri window or Python environment.
pub fn build_segments(
    runner_words: &[RunnerWord],
    diarization: &[SpeakerSegment],
    options: &CrisperOptions,
) -> Vec<TranscriptSegment> {
    let mut drop_flags = Vec::with_capacity(runner_words.len());
    let mut words = Vec::with_capacity(runner_words.len());

    for word in runner_words {
        drop_flags.push(
            (options.remove_fillers && word.filler)
                || (options.remove_vocal_events && word.vocal_event),
        );

        // CrisperWhisper does not diarize; without Sortformer everything is
        // attributed to a single speaker.
        let speaker = if diarization.is_empty() {
            "Speaker 1".to_string()
        } else {
            speaker_label_for_word(word.start, word.end, diarization)
        };

        words.push(WordWithSpeaker {
            start: word.start,
            end: word.end,
            text: word.text.clone(),
            speaker,
        });
    }

    let runs = partition_words(words, &drop_flags);
    build_transcript_segments_from_runs(&runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sortformer reports boundaries in samples at 16 kHz.
    const SAMPLE_RATE_F32: f32 = 16_000.0;

    fn word(start: f32, end: f32, text: &str) -> WordWithSpeaker {
        WordWithSpeaker {
            start,
            end,
            text: text.into(),
            speaker: "Speaker 1".into(),
        }
    }

    #[test]
    fn partition_words_keeps_everything_when_nothing_is_dropped() {
        let words = vec![word(0.0, 0.2, "so"), word(0.2, 0.4, "we"), word(0.4, 0.6, "ship")];
        let runs = partition_words(words, &[false, false, false]);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 3);
    }

    #[test]
    fn partition_words_breaks_the_run_where_a_filler_is_removed() {
        // "so [UM] we ship" -> two runs, so the filler's time span falls
        // between the resulting segments and is cut from the video.
        let words = vec![
            word(0.0, 0.2, "so"),
            word(0.2, 0.7, "[UM]"),
            word(0.7, 0.9, "we"),
            word(0.9, 1.1, "ship"),
        ];
        let runs = partition_words(words, &[false, true, false, false]);

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len(), 1);
        assert_eq!(runs[0][0].text, "so");
        assert_eq!(runs[1].len(), 2);
        assert_eq!(runs[1][0].text, "we");
    }

    #[test]
    fn partition_words_drops_leading_and_trailing_fillers_without_empty_runs() {
        let words = vec![
            word(0.0, 0.4, "[UM]"),
            word(0.4, 0.6, "hello"),
            word(0.6, 1.0, "[laughter]"),
        ];
        let runs = partition_words(words, &[true, false, true]);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 1);
        assert_eq!(runs[0][0].text, "hello");
    }

    #[test]
    fn removed_filler_span_is_excluded_from_the_built_segments() {
        let words = vec![
            word(0.0, 0.4, "Wir"),
            word(0.45, 1.20, "[UM]"),
            word(1.25, 1.60, "liefern."),
        ];
        let runs = partition_words(words, &[false, true, false]);
        let segments = build_transcript_segments_from_runs(&runs);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Wir");
        assert_eq!(segments[1].text, "liefern.");
        // The gap between the two segments is exactly the excised filler.
        assert_eq!(segments[0].end, "00:00.400");
        assert_eq!(segments[1].start, "00:01.250");
        assert!(segments
            .iter()
            .all(|segment| !segment.text.contains("[UM]")));
    }

    #[test]
    fn keeping_fillers_leaves_them_in_the_transcript() {
        let words = vec![
            word(0.0, 0.4, "Wir"),
            word(0.45, 1.20, "[UM]"),
            word(1.25, 1.60, "liefern."),
        ];
        let runs = partition_words(words, &[false, false, false]);
        let segments = build_transcript_segments_from_runs(&runs);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Wir [UM] liefern.");
    }

    #[test]
    fn resolve_extra_only_allows_ct2_where_wheels_exist() {
        let resolved = resolve_extra("ct2");
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!(resolved, "ct2");
        } else {
            assert_eq!(resolved, "transformers");
        }
        assert_eq!(resolve_extra(""), "transformers");
        assert_eq!(resolve_extra("transformers"), "transformers");
    }

    #[test]
    fn venv_python_uses_the_platform_layout() {
        let path = venv_python(Path::new("/tmp/env"));
        if cfg!(target_os = "windows") {
            assert!(path.ends_with("Scripts/python.exe") || path.ends_with("Scripts\\python.exe"));
        } else {
            assert!(path.ends_with("bin/python3"));
        }
    }

    #[test]
    fn options_fall_back_to_documented_defaults() {
        let options = CrisperOptions::default();
        assert_eq!(options.model_or_default(), "large");
        assert_eq!(options.language_or_default(), "en");
        assert_eq!(options.mode_or_default(), "verbatim");
    }

    /// Real CrisperWhisper 2.0 `large` output for
    /// `dev-resources/test-data/test_podcast.m4a`. Using a captured run keeps
    /// these assertions honest about the model's actual token shapes (`[UH]`,
    /// punctuation attached to words) without needing Python or weights.
    const REAL_OUTPUT: &str =
        include_str!("../../dev-resources/test-data/crisperwhisper_large_words.json");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        duration: f32,
        text: String,
        words: Vec<RunnerWord>,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(REAL_OUTPUT).expect("fixture parses")
    }

    fn seconds(timestamp: &str) -> f64 {
        timestamp
            .split(':')
            .fold(0.0, |total, part| total * 60.0 + part.parse::<f64>().unwrap())
    }

    #[test]
    fn real_model_output_maps_to_a_coherent_transcript() {
        let fixture = fixture();
        let segments = build_segments(&fixture.words, &[], &CrisperOptions::default());

        assert!(!segments.is_empty());
        // Every segment must be non-empty, forward-ordered, and inside the audio.
        let mut previous_end = 0.0;
        for segment in &segments {
            let start = seconds(&segment.start);
            let end = seconds(&segment.end);
            assert!(!segment.text.trim().is_empty());
            assert!(end >= start, "segment ends before it starts: {segment:?}");
            assert!(
                start >= previous_end - 0.001,
                "segments overlap or go backwards at {segment:?}"
            );
            assert!(end <= fixture.duration as f64 + 1.0);
            assert!(segment.words.as_ref().is_some_and(|words| !words.is_empty()));
            previous_end = end;
        }

        // Without diarization every word belongs to one speaker.
        assert!(segments.iter().all(|segment| segment.speaker == "Speaker 1"));

        // The joined transcript should carry the recording's distinctive content.
        let joined = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        for phrase in ["unpack this", "deep dive", "media cutter", "open source"] {
            assert!(joined.contains(phrase), "transcript lost {phrase:?}");
        }
    }

    #[test]
    fn real_model_output_carries_the_recordings_fillers_verbatim() {
        let fixture = fixture();
        let fillers: Vec<&RunnerWord> =
            fixture.words.iter().filter(|word| word.filler).collect();

        // The recording contains two audible "uh"s.
        assert_eq!(fillers.len(), 2, "expected two fillers in this recording");
        assert!(fillers
            .iter()
            .all(|word| word.text.eq_ignore_ascii_case("[UH]")));
        assert!(fixture.text.contains("[UH]"));
        // Each filler has a real, non-zero span to cut.
        assert!(fillers.iter().all(|word| word.end > word.start));
    }

    #[test]
    fn removing_fillers_from_real_output_excises_their_spans() {
        let fixture = fixture();
        let filler_spans: Vec<(f32, f32)> = fixture
            .words
            .iter()
            .filter(|word| word.filler)
            .map(|word| (word.start, word.end))
            .collect();

        let kept = build_segments(&fixture.words, &[], &CrisperOptions::default());
        let stripped = build_segments(
            &fixture.words,
            &[],
            &CrisperOptions {
                remove_fillers: true,
                ..Default::default()
            },
        );

        assert!(kept.iter().any(|segment| segment.text.contains("[UH]")));
        assert!(stripped
            .iter()
            .all(|segment| !segment.text.contains("[UH]")));

        // Splitting at each filler yields more segments to cut between.
        assert!(
            stripped.len() > kept.len(),
            "removal should split segments ({} -> {})",
            kept.len(),
            stripped.len()
        );

        // The decisive check: no remaining segment span covers a filler, so the
        // video export physically drops that audio.
        for (start, end) in &filler_spans {
            let midpoint = ((start + end) / 2.0) as f64;
            assert!(
                !stripped.iter().any(|segment| {
                    midpoint > seconds(&segment.start) && midpoint < seconds(&segment.end)
                }),
                "filler at {start}-{end}s still inside a segment span"
            );
        }

        // Real words must survive the removal.
        let joined = stripped
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        for phrase in ["really trying to redefine", "editorial"] {
            assert!(joined.contains(phrase), "removal dropped {phrase:?}");
        }
    }

    #[test]
    fn diarization_labels_real_output_by_speaker() {
        let fixture = fixture();
        // Two speakers splitting the recording at the halfway mark.
        let midpoint = fixture.duration / 2.0;
        let diarization = vec![
            SpeakerSegment {
                speaker_id: 0,
                start: 0,
                end: (midpoint * SAMPLE_RATE_F32) as u64,
            },
            SpeakerSegment {
                speaker_id: 1,
                start: (midpoint * SAMPLE_RATE_F32) as u64,
                end: (fixture.duration * SAMPLE_RATE_F32) as u64,
            },
        ];

        let segments = build_segments(&fixture.words, &diarization, &CrisperOptions::default());
        let speakers: std::collections::BTreeSet<&str> = segments
            .iter()
            .map(|segment| segment.speaker.as_str())
            .collect();

        assert_eq!(
            speakers,
            ["Speaker 1", "Speaker 2"].into_iter().collect(),
            "both diarized speakers should appear"
        );
        // A segment never straddles a speaker change.
        for segment in &segments {
            let words = segment.words.as_ref().unwrap();
            assert!(words
                .iter()
                .all(|word| word.speaker.as_deref() == Some(segment.speaker.as_str())));
        }
    }

    #[test]
    fn vocal_event_removal_is_independent_of_filler_removal() {
        // This recording has no vocal events, so removing them must be a no-op
        // while filler removal still applies.
        let fixture = fixture();
        assert!(fixture.words.iter().all(|word| !word.vocal_event));

        let baseline = build_segments(&fixture.words, &[], &CrisperOptions::default());
        let vocal_only = build_segments(
            &fixture.words,
            &[],
            &CrisperOptions {
                remove_vocal_events: true,
                ..Default::default()
            },
        );

        assert_eq!(baseline.len(), vocal_only.len());
        assert!(vocal_only.iter().any(|segment| segment.text.contains("[UH]")));
    }

    #[test]
    fn embedded_runner_script_is_present_and_declares_the_protocol() {
        assert!(RUNNER_SOURCE.contains("def transcribe"));
        assert!(RUNNER_SOURCE.contains("\"type\": \"result\""));
        // The filler vocabulary must stay in sync with the model's
        // added_tokens.json.
        assert!(RUNNER_SOURCE.contains("[um]"));
        assert!(RUNNER_SOURCE.contains("[laughter]"));
    }
}
