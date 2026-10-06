//! Apple Speech (SpeechAnalyzer) transcription, macOS 26 and newer.
//!
//! macOS ships an on-device speech model behind the `SpeechAnalyzer` /
//! `SpeechTranscriber` APIs: no Python, no model download beyond a per-locale
//! system asset (~13 s for German here), ~45 locales, and word timings with
//! per-word confidence. Measured on an M3 Pro it transcribes ~45-57x faster
//! than real time. It is not verbatim: fillers ("uh", "Mhm") are dropped, so
//! CrisperWhisper stays the engine for cutting out disfluencies.
//!
//! The API is Swift-only and async, so a small Swift helper
//! (`apple-speech/main.swift`, compiled by `build.rs`) is embedded in the
//! binary, written to the app-data directory on first use, and driven out of
//! process with the same JSON-lines protocol as the CrisperWhisper runner. Only
//! the helper needs macOS 26; the app itself keeps running on older systems.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

use crate::error::AppError;
use crate::helper_process::run_json_helper;
use crate::local_asr::{
    build_transcript_segments, diarize, emit_progress, load_audio_16k_mono,
    resolve_sortformer_file, speaker_label_for_word, write_wav_16k_mono, WordWithSpeaker,
};
use crate::run_control::{run_blocking, RunControl};
use crate::video::TranscriptSegment;

const LABEL: &str = "Apple Speech";

/// SpeechAnalyzer first shipped with macOS 26.
const MINIMUM_MACOS_MAJOR: u32 = 26;

#[cfg(apple_speech_helper)]
const HELPER_BINARY: &[u8] = include_bytes!(env!("APPLE_SPEECH_HELPER"));
#[cfg(apple_speech_helper)]
const HELPER_FILE_NAME: &str = "apple-speech-helper";

#[derive(Debug, Clone, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechStatus {
    /// macOS with the helper built in. The engine is not offered elsewhere.
    pub supported_platform: bool,
    /// The transcriber can run on this Mac right now.
    pub available: bool,
    /// BCP 47 identifiers, e.g. `de-DE`.
    pub supported_locales: Vec<String>,
    /// Locales whose on-device model is already downloaded.
    pub installed_locales: Vec<String>,
    /// Why the engine is unavailable, when it is.
    pub message: Option<String>,
}

#[derive(Debug, Default, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", default)]
pub struct AppleSpeechOptions {
    /// BCP 47 locale, e.g. `de-DE`. The model for it is downloaded on first use.
    pub locale: String,
    /// Assign speakers with Sortformer (the transcriber does not diarize).
    pub diarize: bool,
    pub sortformer_model_path: String,
}

/// One timed word as reported by the helper.
#[derive(Debug, Clone, Deserialize)]
pub struct HelperWord {
    pub text: String,
    pub start: f32,
    pub end: f32,
}

#[derive(Debug, Deserialize)]
struct HelperResult {
    #[serde(default)]
    words: Vec<HelperWord>,
}

/// The helper's `probe` reply.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ProbeReply {
    available: bool,
    supported_locales: Vec<String>,
    installed_locales: Vec<String>,
}

fn unsupported(message: &str) -> AppleSpeechStatus {
    AppleSpeechStatus {
        message: Some(message.to_string()),
        ..Default::default()
    }
}

/// Major macOS version, from `sw_vers` (`None` off macOS or if it fails).
fn macos_major_version() -> Option<u32> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    parse_major_version(&String::from_utf8_lossy(&output.stdout))
}

fn parse_major_version(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// Write the embedded helper to the app-data directory, replacing it only
/// when the embedded binary changed.
#[cfg(apple_speech_helper)]
fn helper_path(window: &tauri::Window) -> Result<PathBuf> {
    use anyhow::Context;
    use std::os::unix::fs::PermissionsExt;
    use tauri::Manager;

    let directory = window
        .path()
        .app_data_dir()
        .map_err(|error| anyhow!(error.to_string()))?
        .join("helpers");
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("Failed to create '{}'", directory.display()))?;

    let path = directory.join(HELPER_FILE_NAME);
    if std::fs::read(&path).ok().as_deref() != Some(HELPER_BINARY) {
        // Write beside it and rename, so a helper that is still running from
        // an earlier call is never overwritten in place.
        let staging = directory.join(format!("{HELPER_FILE_NAME}.{}.tmp", fastrand::u64(..)));
        std::fs::write(&staging, HELPER_BINARY)
            .with_context(|| format!("Failed to write '{}'", staging.display()))?;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&staging, &path)
            .with_context(|| format!("Failed to install '{}'", path.display()))?;
    }
    Ok(path)
}

#[cfg(not(apple_speech_helper))]
fn helper_path(_window: &tauri::Window) -> Result<PathBuf> {
    Err(anyhow!(
        "This build does not include the Apple Speech helper."
    ))
}

/// The helper, or the reason the engine cannot run on this machine.
fn usable_helper(window: &tauri::Window) -> std::result::Result<PathBuf, AppleSpeechStatus> {
    if !cfg!(target_os = "macos") {
        return Err(unsupported("Apple Speech is only available on macOS."));
    }
    if !cfg!(apple_speech_helper) {
        return Err(unsupported(
            "This build does not include the Apple Speech helper (it needs the macOS 26 SDK).",
        ));
    }
    match macos_major_version() {
        Some(major) if major < MINIMUM_MACOS_MAJOR => {
            return Err(AppleSpeechStatus {
                supported_platform: true,
                message: Some(format!(
                    "Apple Speech needs macOS {MINIMUM_MACOS_MAJOR} or newer; this Mac runs macOS {major}."
                )),
                ..Default::default()
            })
        }
        _ => {}
    }
    helper_path(window).map_err(|error| AppleSpeechStatus {
        supported_platform: true,
        message: Some(format!("{error:#}")),
        ..Default::default()
    })
}

async fn call_helper(
    window: &tauri::Window,
    helper: &Path,
    request: serde_json::Value,
    run: Option<(u64, &RunControl)>,
) -> Result<serde_json::Value> {
    run_json_helper(
        LABEL,
        &|text| {
            let _ = emit_progress(window, text);
        },
        tokio::process::Command::new(helper),
        request,
        run,
    )
    .await
}

async fn probe(window: &tauri::Window) -> AppleSpeechStatus {
    let helper = match usable_helper(window) {
        Ok(helper) => helper,
        Err(status) => return status,
    };
    let response = call_helper(
        window,
        &helper,
        serde_json::json!({ "action": "probe" }),
        None,
    )
    .await;
    match response.and_then(|value| Ok(serde_json::from_value::<ProbeReply>(value)?)) {
        Ok(reply) => AppleSpeechStatus {
            supported_platform: true,
            available: reply.available,
            supported_locales: reply.supported_locales,
            installed_locales: reply.installed_locales,
            message: (!reply.available).then(|| {
                "Apple Speech reports that on-device transcription is unavailable on this Mac."
                    .to_string()
            }),
        },
        Err(error) => AppleSpeechStatus {
            supported_platform: true,
            message: Some(format!("{error:#}")),
            ..Default::default()
        },
    }
}

/// Whether Apple Speech can run here, and for which locales.
#[tauri::command]
#[specta::specta]
pub async fn apple_speech_status(window: tauri::Window) -> Result<AppleSpeechStatus, AppError> {
    Ok(probe(&window).await)
}

/// Download the on-device model for `locale` (a macOS system asset).
#[tauri::command]
#[specta::specta]
pub async fn install_apple_speech_locale(
    window: tauri::Window,
    locale: String,
) -> Result<AppleSpeechStatus, AppError> {
    let helper =
        usable_helper(&window).map_err(|status| anyhow!(status.message.unwrap_or_default()))?;
    call_helper(
        &window,
        &helper,
        serde_json::json!({ "action": "install", "locale": locale.trim() }),
        None,
    )
    .await?;
    Ok(probe(&window).await)
}

/// Transcribe with Apple Speech and return editor-ready segments.
#[tauri::command]
#[specta::specta]
pub async fn transcribe_with_apple_speech(
    run_id: u64,
    window: tauri::Window,
    audio_path: String,
    options: AppleSpeechOptions,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, AppError> {
    let run_control = run_control.inner();
    let check = || {
        run_control
            .ensure_active(run_id)
            .map_err(|error| anyhow!(error))
    };
    let run = async {
        check()?;
        let audio_file = PathBuf::from(&audio_path);
        if !audio_file.exists() {
            return Err(anyhow!("Audio file not found: {}", audio_file.display()));
        }
        let locale = options.locale.trim();
        if locale.is_empty() {
            return Err(anyhow!("Choose a language for Apple Speech in Settings."));
        }
        let helper =
            usable_helper(&window).map_err(|status| anyhow!(status.message.unwrap_or_default()))?;

        // Resolve the diarization model before transcribing so a missing
        // download fails fast.
        let sortformer_file = if options.diarize {
            Some(resolve_sortformer_file(&window, &options.sortformer_model_path).await?)
        } else {
            None
        };

        // AVAudioFile cannot read most video containers; one FFmpeg pass gives
        // the helper a WAV and Sortformer the same samples.
        emit_progress(&window, "Preparing audio for Apple Speech...")?;
        let wav_path = std::env::temp_dir().join(format!(
            "ai-media-cutter-apple-speech-{}.wav",
            fastrand::u64(..)
        ));
        {
            let (source, destination) = (audio_file.clone(), wav_path.clone());
            run_blocking(move || {
                write_wav_16k_mono(&source, &destination).map_err(|error| format!("{error:#}"))
            })
            .await
            .map_err(|error| anyhow!(error))?;
        }

        let outcome = async {
            check()?;
            let response = call_helper(
                &window,
                &helper,
                serde_json::json!({
                    "action": "transcribe",
                    "locale": locale,
                    "audioPath": wav_path.to_string_lossy(),
                }),
                Some((run_id, run_control)),
            )
            .await?;
            let result: HelperResult = serde_json::from_value(response)?;

            let diarization = match sortformer_file {
                Some(file) if !result.words.is_empty() => {
                    check()?;
                    emit_progress(&window, "Running Sortformer diarization...")?;
                    let wav_path = wav_path.clone();
                    run_blocking(move || {
                        load_audio_16k_mono(&wav_path)
                            .and_then(|audio| diarize(&file, audio))
                            .map_err(|error| format!("{error:#}"))
                    })
                    .await
                    .map_err(|error| anyhow!(error))?
                }
                _ => Vec::new(),
            };

            emit_progress(&window, "Building the Apple Speech transcript...")?;
            Ok(build_segments(&result.words, &diarization))
        }
        .await;

        let _ = std::fs::remove_file(&wav_path);
        outcome
    };

    run.await.map_err(AppError::from)
}

/// Turn the helper's timed words into editor-ready segments.
pub fn build_segments(
    words: &[HelperWord],
    diarization: &[parakeet_rs::sortformer::SpeakerSegment],
) -> Vec<TranscriptSegment> {
    let words: Vec<WordWithSpeaker> = words
        .iter()
        .map(|word| WordWithSpeaker {
            start: word.start,
            end: word.end.max(word.start),
            text: word.text.clone(),
            // The transcriber does not diarize; without Sortformer everything
            // is attributed to a single speaker.
            speaker: if diarization.is_empty() {
                "Speaker 1".to_string()
            } else {
                speaker_label_for_word(word.start, word.end, diarization)
            },
        })
        .collect();
    build_transcript_segments(&words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f32, end: f32) -> HelperWord {
        HelperWord {
            text: text.to_string(),
            start,
            end,
        }
    }

    #[test]
    fn parses_macos_major_versions() {
        assert_eq!(parse_major_version("27.0\n"), Some(27));
        assert_eq!(parse_major_version("15.6.1"), Some(15));
        assert_eq!(parse_major_version(""), None);
    }

    #[test]
    fn builds_single_speaker_segments_from_helper_words() {
        let words = [
            word("Let's", 0.0, 0.36),
            word("unpack", 0.36, 0.6),
            word("this.", 0.6, 0.9),
            // A run whose range ends before it starts must not produce a
            // negative-length word.
            word("We've", 3.0, 2.9),
            word("got", 3.3, 3.4),
        ];
        let segments = build_segments(&words, &[]);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Let's unpack this. We've got");
        assert_eq!(segments[0].speaker, "Speaker 1");
        let timed = segments[0].words.as_ref().unwrap();
        assert_eq!(timed[3].start, timed[3].end);
    }

    #[test]
    fn helper_result_parses_the_wire_format() {
        let result: HelperResult = serde_json::from_value(serde_json::json!({
            "type": "result",
            "locale": "de-DE",
            "words": [{ "text": "Hallo", "start": 0.0, "end": 0.4, "confidence": 0.98 }],
        }))
        .unwrap();
        assert_eq!(result.words.len(), 1);
        assert_eq!(result.words[0].text, "Hallo");
    }

    /// Runs the embedded helper on the known recording through the same
    /// process runner and parsing as the app. Skips where the transcriber or
    /// its English model is not installed (it never downloads one).
    #[cfg(apple_speech_helper)]
    #[tokio::test]
    async fn embedded_helper_transcribes_the_known_recording() {
        use std::os::unix::fs::PermissionsExt;

        if macos_major_version().is_some_and(|major| major < MINIMUM_MACOS_MAJOR) {
            println!("Skipping: macOS {MINIMUM_MACOS_MAJOR}+ required.");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join(HELPER_FILE_NAME);
        std::fs::write(&helper, HELPER_BINARY).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |request: serde_json::Value| {
            run_json_helper(
                LABEL,
                &|_| {},
                tokio::process::Command::new(&helper),
                request,
                None,
            )
        };

        let probe: ProbeReply =
            serde_json::from_value(run(serde_json::json!({ "action": "probe" })).await.unwrap())
                .unwrap();
        if !probe.available
            || !probe
                .installed_locales
                .iter()
                .any(|locale| locale == "en-US")
        {
            println!("Skipping: Apple Speech or its en-US model is not installed.");
            return;
        }

        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../dev-resources/test-data/test_podcast.m4a");
        let wav = directory.path().join("test.wav");
        write_wav_16k_mono(&source, &wav).unwrap();

        let response = run(serde_json::json!({
            "action": "transcribe",
            "locale": "en-US",
            "audioPath": wav.to_string_lossy(),
        }))
        .await
        .unwrap();
        let result: HelperResult = serde_json::from_value(response).unwrap();
        assert!(
            result.words.len() > 150,
            "only {} words",
            result.words.len()
        );
        assert!(result
            .words
            .windows(2)
            .all(|pair| pair[1].start >= pair[0].start - 0.001));

        let text = build_segments(&result.words, &[])
            .iter()
            .map(|segment| segment.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        for phrase in ["unpack this", "deep dive", "media cutter", "video editing"] {
            assert!(text.contains(phrase), "missing {phrase:?} in {text}");
        }
    }

    #[test]
    fn probe_reply_tolerates_missing_fields() {
        let reply: ProbeReply = serde_json::from_value(serde_json::json!({
            "type": "result",
            "available": true,
            "supportedLocales": ["de-DE", "en-US"],
        }))
        .unwrap();
        assert!(reply.available);
        assert_eq!(reply.supported_locales, ["de-DE", "en-US"]);
        assert!(reply.installed_locales.is_empty());
    }
}
